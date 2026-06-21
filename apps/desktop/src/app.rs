use app_services::AppServices;
use core_domain::{
    ConnectionProfile, ConnectionProfileInput, DatabaseKind, ExplorerNode, ExplorerNodeType,
    QueryCellValue, QueryExecution, QueryResult, SavedQueryEntry, SslMode, TableDefinition,
    TableRef,
};
use eframe::egui::{
    self, Align2, Color32, FontFamily, FontId, RichText, Stroke, TextEdit, TextFormat, Vec2,
};
use egui_extras::{Size, StripBuilder, TableBuilder};
use rfd::FileDialog;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

pub struct DesktopApp {
    runtime: Runtime,
    services: AppServices,
    connections: Vec<ConnectionProfile>,
    roots_by_connection: HashMap<String, Vec<ExplorerNode>>,
    children_by_node: HashMap<String, Vec<ExplorerNode>>,
    expanded_nodes: HashSet<String>,
    selected_tree_item: Option<String>,
    selected_connection: Option<String>,
    search_keyword: String,
    tabs: Vec<WorkspaceTab>,
    active_tab: usize,
    status_message: String,
    status_level: StatusLevel,
    sidebar_width: f32,
    is_connection_dialog_open: bool,
    editing_connection_id: Option<String>,
    connection_form: ConnectionFormState,
    use_dark_theme: bool,
    icon_texture: Option<egui::TextureHandle>,
    pending_connection_tree: Option<Receiver<ConnectionTreeLoadResult>>,
    pending_query_execution: Option<Receiver<QueryExecutionLoadResult>>,
    pending_refresh_active_table: Option<bool>, // Some(true) = reload definition
    sidebar_has_focus: bool,
    sidebar_drag_source: Option<String>,       // 正在被拖拽的连接 id
    sidebar_drag_y: f32,                       // 拖拽时鼠标 Y 坐标
    pending_delete_confirmation: Option<PendingDeleteConfirmation>,
    pending_saved_query_dialog: Option<SavedQueryDialogState>,
    pending_saved_query_delete: Option<PendingSavedQueryDelete>,
    tab_drag_source: Option<usize>,
    tab_drag_target: Option<usize>,
    database_cache: HashMap<String, Vec<String>>,
    pending_database_list: Option<Receiver<DatabaseListResult>>,
    pending_table_preview: Option<Receiver<TablePreviewLoadResult>>,
}

struct ConnectionTreeLoadResult {
    connection_id: String,
    result: Result<Vec<ExplorerNode>, String>,
}

struct DatabaseListResult {
    connection_id: String,
    databases: Result<Vec<String>, String>,
}

struct QueryExecutionLoadResult {
    tab_id: String,
    connection_id: String,
    sql: String,
    statements: Vec<String>,
    results: Vec<QueryResult>,
    error: Option<String>,
}

struct TablePreviewLoadResult {
    tab_id: String,
    definition: Option<Result<TableDefinition, String>>,
    preview: Result<QueryResult, String>,
    /// If true, the caller wanted definition reloaded (clears error on success)
    reloaded_definition: bool,
}

#[derive(Clone)]
enum WorkspaceTab {
    Query(QueryTabState),
    Table(TableTabState),
}

#[derive(Clone)]
struct QueryTabState {
    id: String,
    title: String,
    connection_id: Option<String>,
    database: Option<String>,
    sql: String,
    cursor_range: Option<egui::text::CCursorRange>,
    result: Option<QueryResult>,
    history: Vec<String>,
    saved_queries: Vec<SavedQueryEntry>,
    messages: Vec<String>,
    error: Option<String>,
    active_bottom_tab: QueryBottomTab,
    last_executed_sql: Option<String>,
    result_sort: TableSortState,
    selected_columns: BTreeSet<String>,
    multi_results: Vec<QueryResult>,
    selected_result_index: usize,
    editor_focus_requested: bool,
    editor_height: Option<f32>,
    saved_queries_panel_visible: bool,
    saved_queries_filter: String,
    selected_saved_query_id: Option<String>,
}

#[derive(Clone)]
struct TableTabState {
    id: String,
    table: TableRef,
    title: String,
    database_kind: DatabaseKind,
    definition: Option<TableDefinition>,
    preview: Option<QueryResult>,
    preview_column_widths: Vec<f32>,
    error: Option<String>,
    active_view: TableViewMode,
    preview_sort: TableSortState,
    preview_filter: TableFilterState,
    show_preview_filter: bool,
    preview_limit_enabled: bool,
    preview_page_size: u32,
    last_preview_sql: Option<String>,
    selected_preview_row: Option<usize>,
    selected_preview_rows: BTreeSet<usize>,
    selection_anchor_row: Option<usize>,
    editing_cell: Option<TableCellEditState>,
    pending_insert_row: Option<BTreeMap<String, QueryCellValue>>,
    selected_columns: BTreeSet<String>,
}

#[derive(Clone, Default)]
struct TableSortState {
    column: Option<String>,
    descending: bool,
}

#[derive(Clone)]
struct TableFilterState {
    clauses: Vec<TableFilterClause>,
}

#[derive(Clone)]
struct TableFilterClause {
    joiner: TableFilterJoiner,
    column: Option<String>,
    operator: TableFilterOperator,
    value: String,
    second_value: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableFilterJoiner {
    And,
    Or,
}

#[derive(Clone, Copy)]
enum TableHeaderSortChoice {
    Ascending,
    Descending,
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum TableFilterOperator {
    #[default]
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Contains,
    NotContains,
    BeginsWith,
    NotBeginsWith,
    EndsWith,
    NotEndsWith,
    IsNull,
    IsNotNull,
    IsEmpty,
    IsNotEmpty,
    Between,
    NotBetween,
    InList,
    NotInList,
    Custom,
}

#[derive(Clone, PartialEq, Eq)]
struct TableCellEditState {
    target: TableEditTarget,
    column: String,
    value: String,
    is_null: bool,
    focus_requested: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableEditTarget {
    ExistingRow(usize),
    PendingInsert,
}

#[derive(Clone)]
struct PendingDeleteConfirmation {
    active_tab: usize,
    table_name: String,
    row_indices: Vec<usize>,
}

#[derive(Clone)]
struct SavedQueryDialogState {
    mode: SavedQueryDialogMode,
    connection_id: String,
    title_input: String,
}

#[derive(Clone)]
enum SavedQueryDialogMode {
    Save,
    Update { entry_id: String },
    Rename {
        entry_id: String,
    },
}

#[derive(Clone)]
struct PendingSavedQueryDelete {
    active_tab: usize,
    entry_id: String,
    connection_id: String,
    title: String,
}

#[derive(Clone)]
struct ConnectionFormState {
    name: String,
    kind: DatabaseKind,
    group_name: String,
    host: String,
    port: u16,
    username: String,
    password: String,
    default_database: String,
    save_password: bool,
    connect_timeout_secs: u64,
    ssl_mode: SslMode,
    ssh_enabled: bool,
    ssh_host: String,
    ssh_port: u16,
    ssh_username: String,
}

impl Default for ConnectionFormState {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: DatabaseKind::MySql,
            group_name: String::new(),
            host: "127.0.0.1".into(),
            port: 3306,
            username: String::new(),
            password: String::new(),
            default_database: String::new(),
            save_password: true,
            connect_timeout_secs: 5,
            ssl_mode: SslMode::Prefer,
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_username: String::new(),
        }
    }
}

impl DesktopApp {
    pub fn new(runtime: Runtime, services: AppServices) -> Self {
        // 启动连接池的 keepalive（此时 tokio runtime 已存在）
        services.start_keepalive();

        let connections = services.list_connections().unwrap_or_default();
        let selected_connection = services.load_ui_state("selected_connection").ok().flatten();
        let sidebar_width = services
            .load_ui_state("sidebar_width")
            .ok()
            .flatten()
            .and_then(|value| value.parse::<f32>().ok())
            .map(|value| value.clamp(180.0, 300.0))
            .unwrap_or(200.0);
        let use_dark_theme = services
            .load_ui_state("theme")
            .ok()
            .flatten()
            .map(|value| value != "light")
            .unwrap_or(true);
        let mut app = Self {
            runtime,
            services,
            connections,
            roots_by_connection: HashMap::new(),
            children_by_node: HashMap::new(),
            expanded_nodes: HashSet::new(),
            selected_tree_item: None,
            selected_connection,
            search_keyword: String::new(),
            tabs: vec![WorkspaceTab::Query(QueryTabState::new(None))],
            active_tab: 0,
            status_message: "就绪".into(),
            status_level: StatusLevel::Normal,
            sidebar_width,
            is_connection_dialog_open: false,
            editing_connection_id: None,
            connection_form: ConnectionFormState::default(),
            use_dark_theme,
            icon_texture: None,
            pending_connection_tree: None,
            pending_query_execution: None,
            pending_refresh_active_table: None,
            sidebar_has_focus: false,
            sidebar_drag_source: None,
            sidebar_drag_y: 0.0,
            pending_delete_confirmation: None,
            pending_saved_query_dialog: None,
            pending_saved_query_delete: None,
            tab_drag_source: None,
            tab_drag_target: None,
            database_cache: HashMap::new(),
            pending_table_preview: None,
            pending_database_list: None,
        };
        if let Some(connection_id) = app.selected_connection.clone() {
            // #region debug-point A:startup-restore-selected-connection
            let started_at = Instant::now();
            debug_report(
                "pre-fix",
                "A",
                "app.rs:new:selected-connection:start",
                "[DEBUG] 启动时恢复已选连接",
                format!("connection_id={connection_id}"),
            );
            // #endregion
            app.request_load_connection_tree(connection_id.clone());
            // 启动时若有选中连接，加载已保存查询到初始 query tab
            if let Some(WorkspaceTab::Query(tab)) = app.tabs.get_mut(0) {
                let (history, saved_queries) = load_query_library(&app.services, &connection_id);
                tab.history = history;
                tab.saved_queries = saved_queries;
            }
            // #region debug-point A:startup-restore-selected-connection
            debug_report(
                "pre-fix",
                "A",
                "app.rs:new:selected-connection:end",
                "[DEBUG] 启动时恢复已选连接结束",
                format!(
                    "connection_id={connection_id};elapsed_ms={}",
                    started_at.elapsed().as_millis()
                ),
            );
            // #endregion
        }
        app
    }

    fn refresh_connections(&mut self) {
        match self.services.list_connections() {
            Ok(connections) => self.connections = connections,
            Err(error) => self.status_message = format!("刷新连接失败: {error}"),
        }
    }

    fn disconnect_connection(&mut self, connection_id: &str) {
        let name = self.connection_name(connection_id);
        self.services.disconnect_connection(connection_id);
        self.collapse_connection_tree(connection_id);
        self.status_message = format!("已关闭 {name}");
    }

    fn connection_name(&self, connection_id: &str) -> String {
        self.connections
            .iter()
            .find(|c| c.id == connection_id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| connection_id.to_string())
    }

    fn open_edit_connection_dialog(&mut self, profile: &ConnectionProfile) {
        self.is_connection_dialog_open = true;
        self.editing_connection_id = Some(profile.id.clone());
        let mut form = ConnectionFormState::from_profile(profile);
        // 如果密码已保存，从安全存储中加载
        if profile.password_saved {
            if let Ok(Some(password)) = self.services.load_password(&profile.id) {
                form.password = password;
            }
        }
        self.connection_form = form;
    }

    fn request_load_connection_tree(&mut self, connection_id: String) {
        self.selected_connection = Some(connection_id.clone());
        self.selected_tree_item = Some(connection_id.clone());
        let _ = self
            .services
            .save_ui_state("selected_connection", &connection_id);
        self.status_message = format!("正在连接 {}...", self.connection_name(&connection_id));
        let services = self.services.clone();
        let handle = self.runtime.handle().clone();
        let (sender, receiver) = mpsc::channel();
        self.pending_connection_tree = Some(receiver);

        // #region debug-point B:load-connection-tree
        let started_at = Instant::now();
        debug_report(
            "pre-fix",
            "B",
            "app.rs:load_connection_tree:start",
            "[DEBUG] 开始加载连接树",
            format!("connection_id={connection_id}"),
        );
        // #endregion
        handle.spawn(async move {
            let result = services
                .load_connection_tree(&connection_id)
                .await
                .map_err(|error| error.to_string());
            let elapsed_ms = started_at.elapsed().as_millis();
            match &result {
                Ok(nodes) => {
                    // #region debug-point B:load-connection-tree
                    debug_report(
                        "pre-fix",
                        "B",
                        "app.rs:load_connection_tree:ok",
                        "[DEBUG] 加载连接树成功",
                        format!(
                            "connection_id={connection_id};root_count={};elapsed_ms={elapsed_ms}",
                            nodes.len()
                        ),
                    );
                    // #endregion
                }
                Err(error) => {
                    // #region debug-point B:load-connection-tree
                    debug_report(
                        "pre-fix",
                        "B",
                        "app.rs:load_connection_tree:err",
                        "[DEBUG] 加载连接树失败",
                        format!(
                            "connection_id={connection_id};elapsed_ms={elapsed_ms};error={error}"
                        ),
                    );
                    // #endregion
                }
            }
            let _ = sender.send(ConnectionTreeLoadResult {
                connection_id: connection_id.clone(),
                result,
            });
        });
    }

    fn request_list_databases(&mut self, connection_id: Option<String>) {
        let Some(connection_id) = connection_id else { return };
        // Skip if already in database cache
        if self.database_cache.contains_key(&connection_id) {
            return;
        }
        // Skip if already pending
        if self.pending_database_list.is_some() {
            return;
        }
        let services = self.services.clone();
        let handle = self.runtime.handle().clone();
        let (sender, receiver) = mpsc::channel();
        self.pending_database_list = Some(receiver);
        handle.spawn(async move {
            let databases = services
                .list_databases(&connection_id)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(DatabaseListResult { connection_id, databases });
        });
    }

    fn poll_background_tasks(&mut self) {
        // Poll database list results
        if let Some(receiver) = self.pending_database_list.take() {
            match receiver.try_recv() {
                Ok(message) => match message.databases {
                    Ok(databases) => {
                        self.database_cache.insert(message.connection_id, databases);
                    }
                    Err(error) => {
                        self.status_message = format!("获取数据库列表失败: {error}");
                    }
                },
                Err(TryRecvError::Empty) => {
                    self.pending_database_list = Some(receiver);
                }
                Err(TryRecvError::Disconnected) => {
                    // task dropped, ignore
                }
            }
        }

        if let Some(receiver) = self.pending_connection_tree.take() {
            match receiver.try_recv() {
                Ok(message) => match message.result {
                    Ok(nodes) => {
                        self.roots_by_connection
                            .insert(message.connection_id.clone(), nodes);
                        self.selected_connection = Some(message.connection_id.clone());
                        self.selected_tree_item = Some(message.connection_id.clone());
                        let name = self.connection_name(&message.connection_id);
                        self.status_message = format!("已刷新连接 {name}");
                        // Also fetch database list for this connection
                        self.request_list_databases(Some(message.connection_id.clone()));
                    }
                    Err(error) => {
                        self.status_message = format!("加载连接失败: {error}");
                    }
                },
                Err(TryRecvError::Empty) => {
                    self.pending_connection_tree = Some(receiver);
                }
                Err(TryRecvError::Disconnected) => {
                    self.status_message = "加载连接中断".into();
                }
            }
        }

        if let Some(receiver) = self.pending_query_execution.take() {
            match receiver.try_recv() {
                Ok(message) => {
                    let services = self.services.clone();
                    if let Some(WorkspaceTab::Query(query_tab)) = self
                        .tabs
                        .iter_mut()
                        .find(|tab| matches!(tab, WorkspaceTab::Query(tab) if tab.id == message.tab_id))
                    {
                        if message.error.is_some() {
                            // 有错误：显示错误信息
                            query_tab.error = message.error.clone();
                            query_tab.active_bottom_tab = QueryBottomTab::Messages;
                            if let Some(error) = &message.error {
                                query_tab.messages.push(format!("执行失败: {error}"));
                                self.status_message = format!("SQL 执行失败: {error}");
                            }
                            // 仍保留已执行成功的部分结果
                            if !message.results.is_empty() {
                                let last = message.results.last().cloned().unwrap();
                                let elapsed = last.elapsed_ms;
                                let rows = last.rows.len();
                                query_tab.result = Some(last);
                                query_tab.multi_results = message.results;
                                query_tab.selected_result_index = query_tab.multi_results.len().saturating_sub(1);
                                query_tab.last_executed_sql = Some(message.sql.clone());
                                query_tab.messages.push(format!(
                                    "部分成功: {} / {} 条语句，{} ms, {} 行",
                                    query_tab.multi_results.len(),
                                    message.statements.len(),
                                    elapsed,
                                    rows
                                ));
                            }
                        } else if message.results.is_empty() {
                            query_tab.error = Some("没有返回任何结果".into());
                            query_tab.active_bottom_tab = QueryBottomTab::Messages;
                            query_tab.messages.push("执行完成，但没有返回结果集".into());
                            self.status_message = "SQL 执行完成，无结果".into();
                        } else {
                            // 成功：保存所有结果
                            let total_elapsed: u128 = message.results.iter().map(|r| r.elapsed_ms).sum();
                            let total_rows: usize = message.results.iter().map(|r| r.rows.len()).sum();
                            query_tab.multi_results = message.results;
                            query_tab.selected_result_index = 0;
                            // 默认显示第一个有列的结果
                            let display_index = query_tab
                                .multi_results
                                .iter()
                                .position(|r| !r.columns.is_empty())
                                .unwrap_or(0);
                            query_tab.selected_result_index = display_index;
                            if let Some(mut result) = query_tab.multi_results.get(display_index).cloned() {
                                apply_saved_table_sort(&mut result, &mut query_tab.result_sort);
                                query_tab.result = Some(result);
                            }
                            let statement_count = message.statements.len();
                            if statement_count > 1 {
                                query_tab.messages.push(format!(
                                    "执行完成: {} 条语句，共 {} ms, {} 行",
                                    statement_count, total_elapsed, total_rows
                                ));
                            } else {
                                query_tab.messages.push(format!(
                                    "执行成功: {} ms, 返回 {} 行",
                                    total_elapsed, total_rows
                                ));
                            }
                            let (history, saved_queries) =
                                load_query_library(&services, &message.connection_id);
                            query_tab.history = history;
                            query_tab.saved_queries = saved_queries;
                            query_tab.last_executed_sql = Some(message.sql.clone());
                            query_tab.error = None;
                            query_tab.active_bottom_tab = QueryBottomTab::Results;
                            self.status_message = "SQL 执行成功".into();
                        }
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.pending_query_execution = Some(receiver);
                }
                Err(TryRecvError::Disconnected) => {
                    self.status_message = "查询执行中断".into();
                }
            }
        }

        // Poll table preview results (async: open_table_tab)
        if let Some(receiver) = self.pending_table_preview.take() {
            match receiver.try_recv() {
                Ok(message) => {
                    if let Some(WorkspaceTab::Table(tab)) = self.tabs.iter_mut().find(|tab| {
                        matches!(tab, WorkspaceTab::Table(t) if t.id == message.tab_id)
                    }) {
                        // Table definition
                        match message.definition {
                            Some(Ok(definition)) => {
                                tab.definition = Some(definition);
                                if tab.error.is_some() {
                                    tab.error = None;
                                }
                            }
                            Some(Err(error)) => {
                                tab.error = Some(error);
                            }
                            None => {}
                        }

                        // Preview data
                        match message.preview {
                            Ok(preview) => {
                                let row_count = preview.rows.len();
                                let elapsed_ms = preview.elapsed_ms;
                                let filter_columns = if preview.columns.is_empty() {
                                    tab.definition
                                        .as_ref()
                                        .map(|definition| {
                                            definition
                                                .columns
                                                .iter()
                                                .map(|column| column.name.clone())
                                                .collect::<Vec<_>>()
                                        })
                                        .filter(|columns| !columns.is_empty())
                                        .unwrap_or_default()
                                } else {
                                    preview.columns.clone()
                                };
                                ensure_table_filter_column(
                                    &mut tab.preview_filter,
                                    &filter_columns,
                                );
                                tab.preview_column_widths =
                                    estimate_result_column_widths(&preview);
                                tab.preview = Some(preview);
                                tab.selected_preview_rows
                                    .retain(|index| *index < row_count);
                                if tab
                                    .selected_preview_row
                                    .is_some_and(|index| index >= row_count)
                                {
                                    tab.selected_preview_row = None;
                                }
                                if tab
                                    .selection_anchor_row
                                    .is_some_and(|index| index >= row_count)
                                {
                                    tab.selection_anchor_row = None;
                                }
                                normalize_preview_selection(tab);
                                if matches!(
                                    tab.editing_cell.as_ref(),
                                    Some(TableCellEditState {
                                        target: TableEditTarget::ExistingRow(index),
                                        ..
                                    }) if *index >= row_count
                                ) {
                                    tab.editing_cell = None;
                                }
                                if message.reloaded_definition && tab.error.is_some() && tab.definition.is_some() {
                                    tab.error = None;
                                } else if !message.reloaded_definition && tab.error.is_some() {
                                    tab.error = None;
                                }
                                self.status_message = format!(
                                    "表预览已刷新: {} 行, {} ms",
                                    row_count, elapsed_ms
                                );
                                self.status_level = StatusLevel::Success;
                            }
                            Err(error) => {
                                tab.error = Some(error.to_string());
                                self.status_message =
                                    format!("表预览刷新失败: {error}");
                                self.status_level = StatusLevel::Error;
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.pending_table_preview = Some(receiver);
                }
                Err(TryRecvError::Disconnected) => {
                    // task dropped, ignore
                }
            }
        }
    }

    fn load_connection_tree(&mut self, connection_id: &str) {
        self.request_load_connection_tree(connection_id.to_string());
    }

    fn selected_sidebar_connection(&self) -> Option<&ConnectionProfile> {
        let selected_id = self.selected_tree_item.as_deref()?;
        self.connections
            .iter()
            .find(|connection| connection.id == selected_id)
    }

    fn selected_sidebar_node(&self) -> Option<ExplorerNode> {
        let selected_id = self.selected_tree_item.as_deref()?;
        self.roots_by_connection
            .values()
            .flatten()
            .find(|node| node.id == selected_id)
            .cloned()
            .or_else(|| {
                self.children_by_node
                    .values()
                    .flatten()
                    .find(|node| node.id == selected_id)
                    .cloned()
            })
    }

        /// Build a SQL-qualified name for a tree node (Table/View → "db"."schema"."table" or "db"."table").
fn sidebar_node_qualified_name(node: &ExplorerNode) -> String {
    match node.node_type {
        ExplorerNodeType::Table | ExplorerNodeType::View => {
            match (&node.database, &node.schema) {
                (Some(db), Some(schema)) => format!("\"{db}\".\"{schema}\".\"{}\"", node.name),
                (Some(db), None) => format!("\"{db}\".\"{}\"", node.name),
                (None, Some(schema)) => format!("\"{schema}\".\"{}\"", node.name),
                (None, None) => format!("\"{}\"", node.name),
            }
        }
        _ => node.name.clone(),
    }
}

    fn sidebar_node_copy_text(node: &ExplorerNode) -> String {
        Self::sidebar_node_qualified_name(node)
    }

    fn copy_sidebar_text(&mut self, ctx: &egui::Context, text: &str) {
        ctx.copy_text(text.to_string());
        self.status_message = format!("已复制 {text}");
    }

    fn copy_selected_sidebar_item(&mut self, ctx: &egui::Context) -> bool {
        if let Some(connection) = self.selected_sidebar_connection() {
            let text = connection.name.clone();
            self.copy_sidebar_text(ctx, &text);
            return true;
        }
        if let Some(node) = self.selected_sidebar_node() {
            let text = Self::sidebar_node_copy_text(&node);
            self.copy_sidebar_text(ctx, &text);
            return true;
        }
        false
    }

    fn clear_column_and_row_selection(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        match tab {
            WorkspaceTab::Query(query_tab) => {
                query_tab.selected_columns.clear();
            }
            WorkspaceTab::Table(table_tab) => {
                table_tab.selected_columns.clear();
                table_tab.selected_preview_row = None;
                table_tab.selected_preview_rows.clear();
                table_tab.selection_anchor_row = None;
            }
        }
        self.status_message = "已清除选择".into();
    }

    fn copy_selected_columns(&mut self, ctx: &egui::Context) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };

        match tab {
            WorkspaceTab::Query(query_tab) => {
                if query_tab.selected_columns.is_empty() {
                    return;
                }
                let Some(ref result) = query_tab.result else { return };
                let selected_cols: Vec<&String> = result
                    .columns
                    .iter()
                    .filter(|c| query_tab.selected_columns.contains(*c))
                    .collect();
                if selected_cols.is_empty() {
                    return;
                }
                let mut lines = Vec::with_capacity(result.rows.len());
                for row in &result.rows {
                    let line: Vec<String> = selected_cols
                        .iter()
                        .map(|col| {
                            row.get(*col)
                                .map(|v| v.as_text().unwrap_or_default().to_string())
                                .unwrap_or_default()
                        })
                        .collect();
                    lines.push(line.join("\t"));
                }
                let text = lines.join("\n");
                let col_count = selected_cols.len();
                let row_count = result.rows.len();
                ctx.copy_text(text);
                self.status_message = format!("已复制 {} 列, {} 行", col_count, row_count);
                query_tab.selected_columns.clear();
            }
            WorkspaceTab::Table(table_tab) => {
                if table_tab.selected_columns.is_empty() {
                    return;
                }
                let Some(ref preview) = table_tab.preview else { return };
                let columns = table_editable_columns(table_tab);
                let selected_cols: Vec<&String> = columns
                    .iter()
                    .filter(|c| table_tab.selected_columns.contains(*c))
                    .collect();
                if selected_cols.is_empty() {
                    return;
                }
                let mut lines = Vec::with_capacity(preview.rows.len());
                for row in &preview.rows {
                    let line: Vec<String> = selected_cols
                        .iter()
                        .map(|col| {
                            row.get(*col)
                                .map(|v| v.as_text().unwrap_or_default().to_string())
                                .unwrap_or_default()
                        })
                        .collect();
                    lines.push(line.join("\t"));
                }
                let text = lines.join("\n");
                let col_count = selected_cols.len();
                let row_count = preview.rows.len();
                ctx.copy_text(text);
                self.status_message = format!("已复制 {} 列, {} 行", col_count, row_count);
                table_tab.selected_columns.clear();
            }
        }
    }

    fn open_selected_sidebar_item(&mut self) -> bool {
        if let Some(node) = self.selected_sidebar_node() {
            if matches!(node.node_type, ExplorerNodeType::Table | ExplorerNodeType::View) {
                self.open_table_tab(&node);
                return true;
            }
        }
        false
    }

    fn collapse_connection_tree(&mut self, connection_id: &str) {
        let mut stack = self
            .roots_by_connection
            .get(connection_id)
            .cloned()
            .unwrap_or_default();

        while let Some(node) = stack.pop() {
            self.expanded_nodes.remove(&node.id);
            if let Some(children) = self.children_by_node.remove(&node.id) {
                stack.extend(children);
            }
        }

        self.roots_by_connection.remove(connection_id);
        self.selected_tree_item = Some(connection_id.to_string());
        let name = self.connection_name(connection_id);
        self.status_message = format!("已折叠连接 {name}");
    }

    fn load_children(&mut self, connection_id: &str, node: &ExplorerNode) {
        // #region debug-point C:load-children
        let started_at = Instant::now();
        debug_report(
            "pre-fix",
            "C",
            "app.rs:load_children:start",
            "[DEBUG] 开始加载节点子级",
            format!(
                "connection_id={connection_id};node_id={};node_name={}",
                node.id, node.name
            ),
        );
        // #endregion
        match self
            .runtime
            .block_on(self.services.load_node_children(connection_id, node))
        {
            Ok(children) => {
                self.children_by_node.insert(node.id.clone(), children);
                self.status_message = format!("已刷新 {}", node.name);
                // #region debug-point C:load-children
                debug_report(
                    "pre-fix",
                    "C",
                    "app.rs:load_children:ok",
                    "[DEBUG] 加载节点子级成功",
                    format!(
                        "connection_id={connection_id};node_id={};child_count={};elapsed_ms={}",
                        node.id,
                        self.children_by_node
                            .get(&node.id)
                            .map(|items| items.len())
                            .unwrap_or_default(),
                        started_at.elapsed().as_millis()
                    ),
                );
                // #endregion
            }
            Err(error) => {
                // #region debug-point C:load-children
                debug_report(
                    "pre-fix",
                    "C",
                    "app.rs:load_children:err",
                    "[DEBUG] 加载节点子级失败",
                    format!(
                        "connection_id={connection_id};node_id={};elapsed_ms={};error={error}",
                        node.id,
                        started_at.elapsed().as_millis()
                    ),
                );
                // #endregion
                self.status_message = format!("加载节点失败: {error}")
            }
        }
    }

    fn open_table_tab(&mut self, node: &ExplorerNode) {
        let table = TableRef {
            connection_id: node.connection_id.clone(),
            database: node.database.clone(),
            schema: node.schema.clone(),
            table: node.name.clone(),
            is_view: matches!(node.node_type, ExplorerNodeType::View),
        };
        let database_kind = self.database_kind_for_connection(&table.connection_id);
        let tab_id = format!("table-{}", uuid::Uuid::new_v4());

        let table_tab = TableTabState {
            id: tab_id.clone(),
            title: table.label(),
            database_kind,
            table: table.clone(),
            definition: None,
            preview: None,
            preview_column_widths: Vec::new(),
            error: None,
            active_view: TableViewMode::Data,
            preview_sort: TableSortState::default(),
            preview_filter: TableFilterState::default(),
            show_preview_filter: false,
            preview_limit_enabled: true,
            preview_page_size: 1000,
            last_preview_sql: None,
            selected_preview_row: None,
            selected_preview_rows: BTreeSet::new(),
            selection_anchor_row: None,
            editing_cell: None,
            pending_insert_row: None,
            selected_columns: BTreeSet::new(),
        };
        self.tabs.push(WorkspaceTab::Table(table_tab));
        self.active_tab = self.tabs.len().saturating_sub(1);

        // Spawn async background task to load definition and preview data
        let services = self.services.clone();
        let handle = self.runtime.handle().clone();
        let (sender, receiver) = mpsc::channel();
        self.pending_table_preview = Some(receiver);
        let display_sql = build_table_preview_display_sql(
            database_kind,
            &table,
            &TableFilterState::default(),
            &TableSortState::default(),
            Some(1000),
        );
        let preview_sql = build_table_preview_sql(
            database_kind,
            &table,
            &TableFilterState::default(),
            &TableSortState::default(),
            Some(1000),
        );
        handle.spawn(async move {
            let definition = services
                .load_table_definition(&table)
                .await
                .map_err(|error| error.to_string());
            let execution = QueryExecution {
                connection_id: table.connection_id.clone(),
                database: table.database.clone(),
                sql: preview_sql,
            };
            let preview = services
                .execute_sql(execution)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(TablePreviewLoadResult {
                tab_id,
                definition: Some(definition),
                preview,
                reloaded_definition: true,
            });
        });
    }

    fn refresh_active_table_preview(&mut self, reload_definition: bool) {
        let Some(WorkspaceTab::Table(tab)) = self.tabs.get_mut(self.active_tab) else {
            return;
        };

        let tab_id = tab.id.clone();
        let table = tab.table.clone();
        let database_kind = tab.database_kind;

        // Show spinner immediately by clearing old data
        tab.error = None;
        tab.preview = None;
        if reload_definition {
            tab.definition = None;
        }
        self.status_message = "正在刷新...".into();
        self.status_level = StatusLevel::Pending;

        let services = self.services.clone();
        let handle = self.runtime.handle().clone();
        let (sender, receiver) = mpsc::channel();
        self.pending_table_preview = Some(receiver);
        let display_sql = build_table_preview_display_sql(
            database_kind,
            &table,
            &tab.preview_filter,
            &tab.preview_sort,
            if tab.preview_limit_enabled {
                Some(tab.preview_page_size.max(1))
            } else {
                None
            },
        );
        let preview_sql = build_table_preview_sql(
            database_kind,
            &table,
            &tab.preview_filter,
            &tab.preview_sort,
            if tab.preview_limit_enabled {
                Some(tab.preview_page_size.max(1))
            } else {
                None
            },
        );
        handle.spawn(async move {
            let definition = if reload_definition {
                Some(
                    services
                        .load_table_definition(&table)
                        .await
                        .map_err(|error| error.to_string()),
                )
            } else {
                None
            };
            let execution = QueryExecution {
                connection_id: table.connection_id.clone(),
                database: table.database.clone(),
                sql: preview_sql,
            };
            let preview = services
                .execute_sql(execution)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(TablePreviewLoadResult {
                tab_id,
                definition,
                preview,
                reloaded_definition: reload_definition,
            });
        });
    }

    fn create_query_tab(
        &mut self,
        connection_id: Option<String>,
        database: Option<String>,
        initial_sql: Option<String>,
    ) {
        let mut tab = QueryTabState::new(connection_id.clone());
        tab.database = database;
        if let Some(sql) = initial_sql {
            tab.sql = sql;
        }
        if let Some(ref connection_id) = connection_id {
            let (history, saved_queries) = load_query_library(&self.services, connection_id);
            tab.history = history;
            tab.saved_queries = saved_queries;
        }
        self.tabs.push(WorkspaceTab::Query(tab));
        self.active_tab = self.tabs.len().saturating_sub(1);
    }

    fn close_workspace_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        self.tabs.remove(index);

        if self.tabs.is_empty() {
            self.tabs
                .push(WorkspaceTab::Query(QueryTabState::new(self.selected_connection.clone())));
            self.active_tab = 0;
            return;
        }

        if self.active_tab > index {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }
    }

    fn test_connection_form(&mut self) {
        let input = self.connection_form.to_input();
        match self.runtime.block_on(self.services.test_connection(input)) {
            Ok(_) => {
                self.status_message = "连接测试成功".into();
                self.status_level = StatusLevel::Success;
            }
            Err(error) => {
                self.status_message = format!("连接测试失败: {error}");
                self.status_level = StatusLevel::Error;
            }
        }
    }

    fn save_connection_form(&mut self) {
        let input = self.connection_form.to_input();
        let result = if let Some(connection_id) = self.editing_connection_id.clone() {
            self.services.update_connection(&connection_id, input)
        } else {
            self.services.save_connection(input)
        };
        match result {
            Ok(_) => {
                self.is_connection_dialog_open = false;
                self.editing_connection_id = None;
                self.connection_form = ConnectionFormState::default();
                self.refresh_connections();
                self.status_message = "连接已保存".into();
            }
            Err(error) => {
                self.status_message = format!("保存连接失败: {error}")
            }
        }
    }

    fn execute_current_query(&mut self, mode: ExecuteMode) {
        if self.pending_query_execution.is_some() {
            self.status_message = "当前已有 SQL 正在执行".into();
            return;
        }

        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let WorkspaceTab::Query(query_tab) = tab else {
            return;
        };
        let connection_id = query_tab
            .connection_id
            .clone()
            .or_else(|| self.selected_connection.clone());
        let Some(connection_id) = connection_id else {
            self.status_message = "请先选择一个连接".into();
            return;
        };
        query_tab.connection_id = Some(connection_id.clone());

        let sql = match mode {
            ExecuteMode::Whole => query_tab.sql.trim().to_string(),
            ExecuteMode::Selection(selected) => {
                if let Some(sql) = selected {
                    if sql.trim().is_empty() {
                        self.status_message = "请先选中要执行的 SQL".into();
                        query_tab
                            .messages
                            .push("未执行：请先在编辑器中选中要执行的 SQL".into());
                        return;
                    }
                    sql
                } else {
                    self.status_message = "请先选中要执行的 SQL".into();
                    query_tab
                        .messages
                        .push("未执行：请先在编辑器中选中要执行的 SQL".into());
                    return;
                }
            }
        };

        if sql.trim().is_empty() {
            self.status_message = "没有可执行的 SQL".into();
            query_tab
                .messages
                .push("未执行：未检测到选中 SQL 或当前语句".into());
            return;
        }

        // 按分号拆分为多条独立语句
        let statements = split_sql_statements(&sql);
        if statements.is_empty() {
            self.status_message = "没有可执行的 SQL".into();
            return;
        }

        if statements.len() > 1 {
            query_tab.messages.push(format!(
                "检测到 {} 条 SQL 语句，将依次执行",
                statements.len()
            ));
        }

        let tab_id = query_tab.id.clone();
        let database = query_tab.database.clone();
        let services = self.services.clone();
        let handle = self.runtime.handle().clone();
        let (sender, receiver) = mpsc::channel();
        self.pending_query_execution = Some(receiver);
        query_tab.messages.push("开始执行 SQL...".into());
        self.status_message = "正在执行 SQL...".into();

        handle.spawn(async move {
            let mut results: Vec<QueryResult> = Vec::new();
            let mut error: Option<String> = None;
            for statement in &statements {
                let execution = QueryExecution {
                    connection_id: connection_id.clone(),
                    database: database.clone(),
                    sql: statement.clone(),
                };
                match services.execute_sql(execution).await {
                    Ok(result) => results.push(result),
                    Err(err) => {
                        error = Some(err.to_string());
                        break;
                    }
                }
            }
            let _ = sender.send(QueryExecutionLoadResult {
                tab_id,
                connection_id,
                sql: statements.join(";\n"),
                statements,
                results,
                error,
            });
        });
    }

    fn export_active_result(&mut self) {
        let Some(path) = FileDialog::new().add_filter("CSV", &["csv"]).save_file() else {
            return;
        };
        let result = match self.tabs.get(self.active_tab) {
            Some(WorkspaceTab::Query(tab)) => tab.result.clone(),
            Some(WorkspaceTab::Table(tab)) => tab.preview.clone(),
            None => None,
        };
        let Some(result) = result else {
            self.status_message = "当前没有可导出的结果".into();
            return;
        };
        match self.services.export_query_result_csv(&result, &path) {
            Ok(_) => self.status_message = format!("已导出到 {}", path.display()),
            Err(error) => self.status_message = format!("导出失败: {error}"),
        }
    }

    fn refresh_active_workspace(&mut self) {
        match self.tabs.get(self.active_tab) {
            Some(WorkspaceTab::Table(_)) => {
                self.refresh_active_table_preview(true);
                self.status_message = "已刷新当前表数据".into();
            }
            Some(WorkspaceTab::Query(_)) => {
                self.execute_current_query(ExecuteMode::Whole);
            }
            None => {
                self.refresh_connections();
                self.status_message = "已刷新连接列表".into();
            }
        }
    }

    fn toolbar_icon(&mut self, ui: &mut egui::Ui) {
        let icon_size = egui::vec2(26.0, 26.0);
        if self.icon_texture.is_none() {
            let icon_data = crate::icon::app_icon_data(64);
            let color_image = egui::ColorImage::from_rgba_unmultiplied(
                [icon_data.width as usize, icon_data.height as usize],
                &icon_data.rgba,
            );
            self.icon_texture = Some(ui.ctx().load_texture(
                "toolbar-icon",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }
        if let Some(ref handle) = self.icon_texture {
            ui.add(
                egui::Image::new(egui::ImageSource::Texture(
                    egui::load::SizedTexture::new(handle.id(), icon_size),
                ))
                .max_size(icon_size),
            );
        }
    }

    fn render_toolbar(&mut self, ui: &mut egui::Ui) {
        let palette = mac_ui_palette(ui.visuals());
        ui.visuals_mut().widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
        ui.horizontal(|ui| {
            self.toolbar_icon(ui);
            ui.separator();
            if toolbar_button(ui, "新建连接", ToolbarButtonKind::Primary).clicked() {
                self.is_connection_dialog_open = true;
                self.editing_connection_id = None;
                self.connection_form = ConnectionFormState::default();
            }
            if toolbar_button(ui, "新建查询", ToolbarButtonKind::Secondary).clicked() {
                self.create_query_tab(self.selected_connection.clone(), None, None);
            }
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let theme_label = if self.use_dark_theme { "切换浅色" } else { "切换深色" };
                if toolbar_button(ui, theme_label, ToolbarButtonKind::Subtle).clicked() {
                    self.use_dark_theme = !self.use_dark_theme;
                    let _ = self
                        .services
                        .save_ui_state("theme", if self.use_dark_theme { "dark" } else { "light" });
                }
                if let Some(connection_id) = &self.selected_connection {
                    let conn_name = self.connection_name(connection_id);
                    let status = self.services.connection_status(connection_id);
                    let dot = match status.state {
                        core_domain::ConnectionState::Connected => palette.success,
                        core_domain::ConnectionState::Failed => palette.danger,
                        core_domain::ConnectionState::Disconnected => palette.muted_dot,
                        core_domain::ConnectionState::Reconnecting => palette.warning,
                    };
                    ui.add_space(8.0);
                    ui.colored_label(dot, "●");
                    ui.label(
                        RichText::new(conn_name.clone())
                            .size(12.0)
                            .color(palette.weak_text),
                    );
                }
            });
        });
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = mac_ui_palette(ui.visuals());
        let mut pending_actions = Vec::new();
        ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 3.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("连接列表").size(12.0).strong().color(palette.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.small(
                    RichText::new(format!("{} 个", self.connections.len())).color(palette.weak_text),
                );
            });
        });
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(palette.search_bg)
            .stroke(Stroke::new(1.0, palette.soft_border))
            .corner_radius(5.0)
            .inner_margin(egui::Margin::symmetric(8, 5))
            .show(ui, |ui| {
                let search_response = ui.add(
                    TextEdit::singleline(&mut self.search_keyword)
                        .hint_text("搜索")
                        .frame(false),
                );
                if search_response.clicked() || search_response.has_focus() {
                    self.sidebar_has_focus = true;
                }
            });
        ui.add_space(6.0);

        egui::ScrollArea::vertical()
            .id_salt("sidebar-tree-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let keyword = self.search_keyword.to_ascii_lowercase();
                let is_searching = !keyword.is_empty();
                let connection_count = self.connections.len();

                // 收集每行 rect
                let mut row_rects: Vec<(String, egui::Rect)> = Vec::new();

                // 按 Escape 取消拖拽
                if self.sidebar_drag_source.is_some()
                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    self.sidebar_drag_source = None;
                }

                for (index, connection) in self.connections.clone().into_iter().enumerate() {
                    // 搜索时只查已打开（已加载树）的连接
                    if is_searching {
                        let has_tree = self.roots_by_connection.contains_key(&connection.id);
                        if !has_tree
                            || (!connection.name.to_ascii_lowercase().contains(&keyword)
                                && !self.tree_contains_keyword(&connection.id, &keyword))
                        {
                            continue;
                        }
                    }
                    let status = self.services.connection_status(&connection.id);
                    let dot = match status.state {
                        core_domain::ConnectionState::Connected => palette.success,
                        core_domain::ConnectionState::Failed => palette.danger,
                        core_domain::ConnectionState::Disconnected => palette.muted_dot,
                        core_domain::ConnectionState::Reconnecting => palette.warning,
                    };
                    let selected = self.selected_tree_item.as_deref() == Some(&connection.id);

                    // 拖拽状态
                    let dragging = self.sidebar_drag_source.as_deref() == Some(&connection.id);

                    // 记录行开始位置
                    let row_start = ui.cursor().min;

                    // 拖拽指示线
                    let dragging_other = self.sidebar_drag_source.is_some() && !dragging;
                    if !is_searching && dragging_other {
                        let ptr_y = ui.input(|i| i.pointer.hover_pos().map(|p| p.y));
                        if let Some(y) = ptr_y {
                            let rect = ui.available_rect_before_wrap();
                            if y >= rect.top() && y < rect.center().y {
                                ui.painter().hline(
                                    rect.x_range(),
                                    rect.top(),
                                    Stroke::new(2.0, palette.accent_button_bg),
                                );
                            }
                        }
                    }

                    // 连接行
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        if dragging {
                            ui.label(
                                RichText::new(node_icon_symbol(ExplorerNodeType::Connection))
                                    .color(palette.muted_dot),
                            );
                        } else {
                            ui.add_sized(
                                [12.0, 18.0],
                                egui::Label::new(
                                    RichText::new(node_icon_symbol(ExplorerNodeType::Connection)).color(
                                        if selected {
                                            palette.selection_text
                                        } else {
                                            palette.weak_text
                                        },
                                    ),
                                ),
                            );
                        }
                        let kind_badge = connection_kind_badge(&connection.kind);
                        let response = tree_row_button(
                            ui,
                            &connection.name,
                            selected && !dragging,
                            true,
                            ui.available_width() - 80.0,
                        );
                        if !dragging {
                            response.context_menu(|ui| {
                                if ui.button("新建查询").clicked() {
                                    let conn_id = connection.id.clone();
                                    self.create_query_tab(Some(conn_id), None, None);
                                    ui.close();
                                }
                                if ui.button("编辑连接").clicked() {
                                    let conn = connection.clone();
                                    self.open_edit_connection_dialog(&conn);
                                    ui.close();
                                }
                                if ui.button("关闭连接").clicked() {
                                    self.disconnect_connection(&connection.id);
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button("刷新").clicked() {
                                    pending_actions.push(SidebarAction::RefreshConnection(
                                        connection.id.clone(),
                                    ));
                                    ui.close();
                                }
                            });
                            if response.clicked() {
                                self.sidebar_has_focus = true;
                                self.selected_connection = Some(connection.id.clone());
                                self.selected_tree_item = Some(connection.id.clone());
                            }
                            if response.double_clicked() {
                                pending_actions
                                    .push(SidebarAction::OpenConnection(connection.id.clone()));
                            }
                            if response.drag_started() {
                                self.sidebar_drag_source = Some(connection.id.clone());
                            }
                        }
                        ui.add_space(2.0);
                        ui.colored_label(dot, "●");
                        ui.label(kind_badge.color(palette.weak_text));
                    });

                    // 记录行结束位置
                    let row_end = ui.cursor().min;
                    row_rects.push((connection.id.clone(), egui::Rect::from_min_max(row_start, row_end)));

                    if let Some(nodes) = self.roots_by_connection.get(&connection.id).cloned() {
                        for node in nodes {
                            self.render_node(ui, &node, 1, &mut pending_actions);
                        }
                    }

                    if let Some(error) = status.last_error {
                        ui.add_space(2.0);
                        ui.small(RichText::new(error).color(palette.danger));
                    }
                    ui.add_space(4.0);
                }

                // 拖拽释放检测（循环结束后统一处理）
                let pointer_released = ui.input(|i| i.pointer.any_released());
                if pointer_released && self.sidebar_drag_source.is_some() {
                    let ptr_y = ui.input(|i| i.pointer.hover_pos().map(|p| p.y));
                    let mut drop_index = None;
                    if let Some(y) = ptr_y {
                        for (idx, (_id, rect)) in row_rects.iter().enumerate() {
                            if y >= rect.top() && y <= rect.bottom() {
                                if y < rect.center().y {
                                    drop_index = Some(idx);
                                } else {
                                    drop_index = Some(idx + 1);
                                }
                                break;
                            }
                        }
                        // 鼠标在所有行下方
                        if drop_index.is_none() {
                            if let Some(last_rect) = row_rects.last() {
                                if y > last_rect.1.bottom() {
                                    drop_index = Some(connection_count);
                                }
                            }
                        }
                    }
                    if let Some(target) = drop_index {
                        self.commit_sidebar_drag_reorder(target);
                    } else {
                        self.sidebar_drag_source = None;
                    }
                }
            });

        let should_repaint = !pending_actions.is_empty();
        for action in pending_actions {
            match action {
                SidebarAction::OpenConnection(connection_id) => {
                    if self.roots_by_connection.contains_key(&connection_id) {
                        self.collapse_connection_tree(&connection_id);
                    } else {
                        self.load_connection_tree(&connection_id);
                    }
                }
                SidebarAction::ToggleNode(connection_id, node) => {
                    if self.expanded_nodes.contains(&node.id) {
                        self.expanded_nodes.remove(&node.id);
                    } else {
                        self.expanded_nodes.insert(node.id.clone());
                        if !self.children_by_node.contains_key(&node.id) {
                            self.load_children(&connection_id, &node);
                        }
                    }
                }
                SidebarAction::OpenTable(node) => self.open_table_tab(&node),
                SidebarAction::RefreshConnection(connection_id) => {
                    self.collapse_connection_tree(&connection_id);
                    self.load_connection_tree(&connection_id);
                    self.status_message = format!(
                        "正在刷新连接 {}...",
                        self.connection_name(&connection_id)
                    );
                }
                SidebarAction::RefreshNode(connection_id, node) => {
                    self.children_by_node.remove(&node.id);
                    // Reload children under this node if it is expanded
                    if self.expanded_nodes.contains(&node.id) {
                        self.load_children(&connection_id, &node);
                        // status_message is set inside load_children on success/error
                    } else {
                        self.status_message = format!("已刷新 {}", node.name);
                    }
                }
            }
        }
        if should_repaint {
            ui.ctx().request_repaint();
        }
    }

    /// 提交拖拽排序
    fn commit_sidebar_drag_reorder(&mut self, target_index: usize) {
        let Some(source_id) = self.sidebar_drag_source.take() else {
            return;
        };
        let source_index = self
            .connections
            .iter()
            .position(|c| c.id == source_id);
        let Some(source_index) = source_index else {
            return;
        };
        if source_index == target_index {
            return;
        }
        // 从旧位置移除，插入到新位置
        let connection = self.connections.remove(source_index);
        let insert_at = if target_index > source_index {
            target_index - 1
        } else {
            target_index
        };
        self.connections.insert(insert_at, connection);

        // 重新分配 sort_order
        let orders: Vec<(String, i64)> = self
            .connections
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i as i64))
            .collect();
        if let Err(error) = self.services.update_sort_orders(&orders) {
            self.status_message = format!("排序更新失败: {error}");
        }
    }

    fn render_node(
        &mut self,
        ui: &mut egui::Ui,
        node: &ExplorerNode,
        depth: usize,
        actions: &mut Vec<SidebarAction>,
    ) {
        let palette = mac_ui_palette(ui.visuals());
        if !self.search_keyword.is_empty()
            && !node.name.to_ascii_lowercase().contains(&self.search_keyword.to_ascii_lowercase())
            && !self.node_or_children_match(node, &self.search_keyword.to_ascii_lowercase())
        {
            return;
        }

        ui.horizontal(|ui| {
            let selected = self.selected_tree_item.as_deref() == Some(&node.id);
            ui.add_space((depth * 12) as f32);
            if node.expandable {
                let is_expanded = self.expanded_nodes.contains(&node.id);
                let expand_response = ui.add(
                    egui::Button::new(
                        RichText::new(if is_expanded { "▾" } else { "▸" }).color(if selected {
                            palette.selection_text
                        } else {
                            palette.weak_text
                        }),
                    )
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(12.0, 18.0)),
                );
                expand_response
                    .clone()
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if expand_response.clicked() {
                    actions.push(SidebarAction::ToggleNode(
                        node.connection_id.clone(),
                        node.clone(),
                    ));
                }
            } else {
                ui.add_sized([12.0, 18.0], egui::Label::new(""));
            }

            ui.add_sized(
                [14.0, 18.0],
                egui::Label::new(
                    RichText::new(node_icon_symbol(node.node_type)).color(if selected {
                        palette.selection_text
                    } else {
                        palette.weak_text
                    }),
                ),
            );
            let response = tree_row_button(
                ui,
                &node.name,
                selected,
                false,
                ui.available_width(),
            );
            response.context_menu(|ui| {
                // 数据库、Schema、表、视图节点可新建查询
                match node.node_type {
                    ExplorerNodeType::Database | ExplorerNodeType::Schema => {
                        let label = match node.node_type {
                            ExplorerNodeType::Schema => "在 Schema 中新建查询",
                            _ => "在库中新建查询",
                        };
                        if ui.button(label).clicked() {
                            let db = node.database.clone();
                            let schema = node.schema.clone();
                            self.create_query_tab(
                                Some(node.connection_id.clone()),
                                db.or_else(|| Some(node.name.clone())),
                                schema.map(|s| format!("-- Schema: {s}\n")),
                            );
                            ui.close();
                        }
                    }
                    ExplorerNodeType::Table | ExplorerNodeType::View => {
                        let label = match node.node_type {
                            ExplorerNodeType::View => "在视图上新建查询",
                            _ => "在表上新建查询",
                        };
                        if ui.button(label).clicked() {
                            let table_name = Self::sidebar_node_qualified_name(node);
                            let db = node.database.clone();
                            let sql = format!(
                                "SELECT *\nFROM {table_name}\nLIMIT 100;\n"
                            );
                            self.create_query_tab(
                                Some(node.connection_id.clone()),
                                db,
                                Some(sql),
                            );
                            ui.close();
                        }
                    }
                    _ => {}
                }
                ui.separator();
                if ui.button("刷新").clicked() {
                    actions.push(SidebarAction::RefreshNode(
                        node.connection_id.clone(),
                        node.clone(),
                    ));
                    ui.close();
                }
                if ui.button("复制").clicked() {
                    let text = Self::sidebar_node_copy_text(node);
                    self.copy_sidebar_text(ui.ctx(), &text);
                    ui.close();
                }
            });
            if response.clicked() {
                self.sidebar_has_focus = true;
                self.selected_connection = Some(node.connection_id.clone());
                self.selected_tree_item = Some(node.id.clone());
            }
            if response.double_clicked() {
                if matches!(node.node_type, ExplorerNodeType::Table | ExplorerNodeType::View) {
                    actions.push(SidebarAction::OpenTable(node.clone()));
                } else if node.expandable {
                    actions.push(SidebarAction::ToggleNode(
                        node.connection_id.clone(),
                        node.clone(),
                    ));
                }
            }
        });

        let force_expand = !self.search_keyword.is_empty();
        if force_expand || self.expanded_nodes.contains(&node.id) {
            if let Some(children) = self.children_by_node.get(&node.id).cloned() {
                for child in children {
                    self.render_node(ui, &child, depth + 1, actions);
                }
            }
        }
    }

    /// 递归检查树中是否有节点名包含关键词
    fn tree_contains_keyword(&self, connection_id: &str, keyword: &str) -> bool {
        if let Some(roots) = self.roots_by_connection.get(connection_id) {
            for root in roots {
                if self.node_or_children_match(root, keyword) {
                    return true;
                }
            }
        }
        false
    }

    fn node_or_children_match(&self, node: &ExplorerNode, keyword: &str) -> bool {
        if node.name.to_ascii_lowercase().contains(keyword) {
            return true;
        }
        if let Some(children) = self.children_by_node.get(&node.id) {
            for child in children {
                if self.node_or_children_match(child, keyword) {
                    return true;
                }
            }
        }
        false
    }

    fn render_tabs(&mut self, ui: &mut egui::Ui) {
        let palette = mac_ui_palette(ui.visuals());
        let mut pending_active_tab = None;
        let mut pending_close_tab = None;
        let mut drag_source = self.tab_drag_source;
        let mut drag_target = self.tab_drag_target;
        egui::Frame::new()
            .fill(palette.toolbar_bg)
            .stroke(Stroke::new(1.0, palette.border))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("workspace-tabs-scroll")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (index, tab) in self.tabs.iter().enumerate() {
                                let (title, icon) = match tab {
                                    WorkspaceTab::Query(tab) => (tab.title.as_str(), tab_icon_symbol(tab)),
                                    WorkspaceTab::Table(tab) => (tab.title.as_str(), tab_icon_symbol(tab)),
                                };
                                let interaction = tab_button(
                                    ui, index, icon, title, self.active_tab == index,
                                );
                                // Drag-and-drop reordering
                                if interaction.tab_response.dragged() {
                                    drag_source = Some(index);
                                    drag_target = None;
                                }
                                if interaction.tab_response.hovered()
                                    && drag_source.is_some()
                                    && drag_source != Some(index)
                                {
                                    drag_target = Some(index);
                                }
                                // Right-click context menu on tabs
                                interaction.tab_response.context_menu(|ui| {
                                    if ui.button("关闭").clicked() {
                                        pending_close_tab = Some(index);
                                        ui.close();
                                    }
                                    if ui.button("关闭其他").clicked() {
                                        pending_close_tab = Some(usize::MAX - 1);
                                        pending_active_tab = Some(index);
                                        ui.close();
                                    }
                                    if ui.button("关闭右侧标签").clicked() {
                                        pending_close_tab = Some(index);
                                        pending_active_tab = Some(index);
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button("关闭全部").clicked() {
                                        pending_close_tab = Some(usize::MAX);
                                        pending_active_tab = Some(usize::MAX);
                                        ui.close();
                                    }
                                });
                                if interaction.close_clicked {
                                    pending_close_tab = Some(index);
                                } else if interaction.tab_clicked {
                                    pending_active_tab = Some(index);
                                }
                            }
                            ui.add_space(4.0);
                        });
                    });
            });
        // Apply drag reorder on drop
        let drag_finished = ui.input(|input| {
            input.pointer.button_released(egui::PointerButton::Primary)
        });
        if drag_finished {
            if let (Some(src), Some(tgt)) = (drag_source, drag_target) {
                if src != tgt {
                    let tab = self.tabs.remove(src);
                    let insert_at = if tgt > src { tgt } else { tgt.max(0) };
                    self.tabs.insert(insert_at, tab);
                    // Update active_tab to follow the moved tab
                    if self.active_tab == src {
                        self.active_tab = insert_at;
                    } else if src < self.active_tab && insert_at >= self.active_tab {
                        self.active_tab -= 1;
                    } else if src > self.active_tab && insert_at <= self.active_tab {
                        self.active_tab += 1;
                    }
                }
            }
            drag_source = None;
            drag_target = None;
        }
        self.tab_drag_source = drag_source;
        self.tab_drag_target = drag_target;
        match (pending_close_tab, pending_active_tab) {
            (Some(usize::MAX), Some(usize::MAX)) => {
                // 关闭全部
                self.tabs.clear();
                self.tabs
                    .push(WorkspaceTab::Query(QueryTabState::new(self.selected_connection.clone())));
                self.active_tab = 0;
                self.request_list_databases(self.selected_connection.clone());
            }
            (Some(val), Some(keep_index)) if val == usize::MAX - 1 => {
                // 关闭其他: keep only the clicked tab
                self.tabs = vec![self.tabs[keep_index].clone()];
                if self.active_tab >= self.tabs.len() {
                    self.active_tab = self.tabs.len().saturating_sub(1);
                }
            }
            (Some(right_index), Some(keep_index)) if right_index == keep_index => {
                // 关闭右侧标签: keep 0..=keep_index, remove all after
                let keep_tabs: Vec<_> = (0..=keep_index)
                    .map(|i| self.tabs[i].clone())
                    .collect();
                self.tabs = keep_tabs;
                if self.active_tab >= self.tabs.len() {
                    self.active_tab = self.tabs.len().saturating_sub(1);
                }
            }
            (Some(pending_close_tab), _) if pending_close_tab != usize::MAX => {
                self.close_workspace_tab(pending_close_tab);
            }
            (_, Some(index)) if index != usize::MAX => {
                self.active_tab = index;
            }
            _ => {}
        }
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(palette.workspace_bg)
            .stroke(Stroke::NONE)
            .inner_margin(egui::Margin::same(0))
            .show(ui, |ui| {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    let action = match tab {
                        WorkspaceTab::Query(tab) => Self::render_query_tab(
                            ui,
                            tab,
                            &self.connections,
                            self.selected_connection.clone(),
                            &self.database_cache,
                            &self.services,
                        ),
                        WorkspaceTab::Table(tab) => Self::render_table_tab(ui, tab),
                    };
                    self.handle_tab_action(ui.ctx(), action);
                }
            });
    }

    fn handle_tab_action(&mut self, ctx: &egui::Context, action: TabUiAction) {
        match action {
            TabUiAction::None => {}
            TabUiAction::ExecuteQuery(mode) => self.execute_current_query(mode),
            TabUiAction::ExportActiveResult => self.export_active_result(),
            TabUiAction::CopyTextToClipboard {
                text,
                status_message,
            } => {
                ctx.copy_text(text);
                self.status_message = status_message;
            }
            TabUiAction::RefreshQueryHistory(connection_id) => {
                let (history, saved_queries) = load_query_library(&self.services, &connection_id);
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.history = history;
                    tab.saved_queries = saved_queries;
                }
            }
            TabUiAction::OpenSaveQueryDialog(connection_id) => {
                self.open_save_query_dialog(&connection_id);
            }
            TabUiAction::OpenRenameSavedQueryDialog(entry) => {
                self.open_rename_saved_query_dialog(&entry);
            }
            TabUiAction::PromptDeleteSavedQuery(entry) => {
                self.request_delete_saved_query(&entry);
            }
            TabUiAction::RefreshActiveTable { reload_definition } => {
                self.pending_refresh_active_table = Some(reload_definition);
                ctx.request_repaint();
            }
            TabUiAction::SaveActiveTableCellEdit {
                row_index,
                column,
                value,
                is_null,
            } => self.save_active_table_cell_edit(row_index, &column, &value, is_null),
            TabUiAction::SavePendingInsertRow => self.save_pending_insert_row(),
            TabUiAction::DeleteActiveTableRows(row_indices) => {
                self.request_delete_active_table_rows(row_indices);
            }
            TabUiAction::CopyActiveTableRowsAsInsert(row_indices) => {
                self.copy_active_table_rows_as_insert(ctx, &row_indices);
            }
            TabUiAction::CopyActiveTableRowsAsTsv(row_indices) => {
                self.copy_active_table_rows_as_tsv(ctx, &row_indices);
            }
            TabUiAction::NewQueryFromTable {
                connection_id,
                database,
                schema,
                table,
            } => {
                let qualified = match (&database, &schema) {
                    (Some(db), Some(s)) => format!("\"{db}\".\"{s}\".\"{table}\""),
                    (Some(db), None) => format!("\"{db}\".\"{table}\""),
                    (None, Some(s)) => format!("\"{s}\".\"{table}\""),
                    (None, None) => format!("\"{table}\""),
                };
                let sql = format!("SELECT *\nFROM {qualified}\nLIMIT 100;\n");
                self.create_query_tab(Some(connection_id), database, Some(sql));
            }
        }
    }

    fn open_save_query_dialog(&mut self, connection_id: &str) {
        let sql = match self.tabs.get(self.active_tab) {
            Some(WorkspaceTab::Query(tab)) => tab.sql.trim().to_string(),
            _ => return,
        };
        if sql.is_empty() {
            self.status_message = "没有可保存的 SQL".into();
            if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                tab.messages.push("保存失败：当前编辑器为空".into());
                tab.active_bottom_tab = QueryBottomTab::Messages;
            }
            return;
        }
        // 检查当前 SQL 是否已经保存过，若是则走更新模式
        let saved_queries = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| match tab {
                WorkspaceTab::Query(t) => Some(t.saved_queries.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let existing = saved_queries.iter().find(|e| e.sql_text.trim() == sql.trim());
        if let Some(entry) = existing {
            self.pending_saved_query_dialog = Some(SavedQueryDialogState {
                mode: SavedQueryDialogMode::Update {
                    entry_id: entry.id.clone(),
                },
                connection_id: connection_id.to_string(),
                title_input: entry.title.clone(),
            });
        } else {
            self.pending_saved_query_dialog = Some(SavedQueryDialogState {
                mode: SavedQueryDialogMode::Save,
                connection_id: connection_id.to_string(),
                title_input: String::new(),
            });
        }
    }

    fn open_rename_saved_query_dialog(&mut self, entry: &SavedQueryEntry) {
        self.pending_saved_query_dialog = Some(SavedQueryDialogState {
            mode: SavedQueryDialogMode::Rename {
                entry_id: entry.id.clone(),
            },
            connection_id: entry.connection_id.clone(),
            title_input: entry.title.clone(),
        });
    }

    fn request_delete_saved_query(&mut self, entry: &SavedQueryEntry) {
        self.pending_saved_query_delete = Some(PendingSavedQueryDelete {
            active_tab: self.active_tab,
            entry_id: entry.id.clone(),
            connection_id: entry.connection_id.clone(),
            title: entry.title.clone(),
        });
    }

    fn save_active_query(&mut self, connection_id: &str, title: &str) {
        let (sql, database) = match self.tabs.get(self.active_tab) {
            Some(WorkspaceTab::Query(tab)) => (tab.sql.trim().to_string(), tab.database.clone()),
            _ => return,
        };
        match self.services.save_query(connection_id, database.as_deref(), title, &sql) {
            Ok(saved) => {
                self.status_message = "已保存当前查询".into();
                let (history, saved_queries) = load_query_library(&self.services, connection_id);
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.connection_id = Some(connection_id.to_string());
                    tab.history = history;
                    tab.saved_queries = saved_queries;
                    tab.messages.push(format!("已保存查询：{}", saved.title));
                    tab.active_bottom_tab = QueryBottomTab::History;
                }
            }
            Err(error) => {
                self.status_message = format!("保存查询失败: {error}");
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.messages.push(format!("保存查询失败: {error}"));
                    tab.active_bottom_tab = QueryBottomTab::Messages;
                }
            }
        }
    }

    fn rename_saved_query(&mut self, entry_id: &str, connection_id: &str, title: &str) {
        match self.services.rename_saved_query(entry_id, title) {
            Ok(()) => {
                self.status_message = "已重命名保存的查询".into();
                let (history, saved_queries) = load_query_library(&self.services, connection_id);
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.history = history;
                    tab.saved_queries = saved_queries;
                    tab.messages.push(format!("已重命名查询：{}", title.trim()));
                    tab.active_bottom_tab = QueryBottomTab::History;
                }
            }
            Err(error) => {
                self.status_message = format!("重命名查询失败: {error}");
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.messages.push(format!("重命名查询失败: {error}"));
                    tab.active_bottom_tab = QueryBottomTab::Messages;
                }
            }
        }
    }

    fn confirm_delete_saved_query(&mut self) {
        let Some(pending) = self.pending_saved_query_delete.take() else {
            return;
        };
        if self.active_tab != pending.active_tab {
            self.status_message = "删除保存查询已过期，请重新操作".into();
            return;
        }
        match self.services.delete_saved_query(&pending.entry_id) {
            Ok(()) => {
                self.status_message = "已删除保存的查询".into();
                let (history, saved_queries) =
                    load_query_library(&self.services, &pending.connection_id);
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.history = history;
                    tab.saved_queries = saved_queries;
                    tab.messages.push(format!("已删除保存查询：{}", pending.title));
                    tab.active_bottom_tab = QueryBottomTab::History;
                }
            }
            Err(error) => {
                self.status_message = format!("删除保存查询失败: {error}");
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.messages.push(format!("删除保存查询失败: {error}"));
                    tab.active_bottom_tab = QueryBottomTab::Messages;
                }
            }
        }
    }

    fn database_kind_for_connection(&self, connection_id: &str) -> DatabaseKind {
        self.connections
            .iter()
            .find(|connection| connection.id == connection_id)
            .map(|connection| connection.kind)
            .unwrap_or(DatabaseKind::MySql)
    }

    fn execute_active_table_mutation(
        &mut self,
        sql: String,
        success_message: impl Into<String>,
    ) -> bool {
        let Some(WorkspaceTab::Table(tab)) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        let connection_id = tab.table.connection_id.clone();
        match self.runtime.block_on(self.services.execute_sql(QueryExecution {
            connection_id,
            database: None,
            sql,
        })) {
            Ok(result) => {
                let affected = result.affected_rows.unwrap_or(0);
                self.status_message = format!("{}，影响 {} 行", success_message.into(), affected);
                tab.error = None;
                true
            }
            Err(error) => {
                let error = error.to_string();
                tab.error = Some(error.clone());
                self.status_message = format!("执行失败: {error}");
                false
            }
        }
    }

    fn save_active_table_cell_edit(
        &mut self,
        row_index: usize,
        column: &str,
        value: &str,
        is_null: bool,
    ) {
        let Some((database_kind, table, definition, row)) = self.active_table_row_context(row_index) else {
            return;
        };
        if table.is_view {
            self.status_message = "视图暂不支持直接编辑".into();
            return;
        }
        let Some(where_clause) =
            build_table_row_match_clause(database_kind, definition.as_ref(), &row, &table)
        else {
            self.status_message = "无法定位当前记录，不能直接更新".into();
            return;
        };
        let sql = format!(
            "UPDATE {}\nSET {} = {}\nWHERE {}",
            qualified_table_name(database_kind, &table),
            quote_identifier(database_kind, column),
            sql_editor_value_literal(value, is_null),
            where_clause
        );
        if self.execute_active_table_mutation(sql, "单元格已更新") {
            if let Some(WorkspaceTab::Table(tab)) = self.tabs.get_mut(self.active_tab) {
                tab.editing_cell = None;
            }
            self.refresh_active_table_preview(false);
        }
    }

    fn save_pending_insert_row(&mut self) {
        let Some(WorkspaceTab::Table(tab)) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.table.is_view {
            self.status_message = "视图暂不支持新增记录".into();
            return;
        }
        let database_kind = tab.database_kind;
        let columns = table_editable_columns(tab);
        let Some(values) = tab.pending_insert_row.as_ref() else {
            self.status_message = "当前没有新增记录".into();
            return;
        };
        let table = tab.table.clone();
        let values = values.clone();
        let Some(sql) =
            build_insert_sql_for_pending_row(database_kind, &table, &columns, &values)
        else {
            self.status_message = "请至少填写一个字段后再保存".into();
            return;
        };
        if self.execute_active_table_mutation(sql, "新增记录成功") {
            if let Some(WorkspaceTab::Table(tab)) = self.tabs.get_mut(self.active_tab) {
                tab.pending_insert_row = None;
                tab.editing_cell = None;
                tab.selected_preview_row = None;
                tab.selected_preview_rows.clear();
                tab.selection_anchor_row = None;
            }
            self.refresh_active_table_preview(false);
        }
    }

    fn delete_active_table_rows(&mut self, row_indices: &[usize]) {
        let Some((database_kind, table, definition, rows)) =
            self.active_table_row_contexts(row_indices)
        else {
            return;
        };
        if table.is_view {
            self.status_message = "视图暂不支持删除记录".into();
            return;
        }
        let where_clauses = rows
            .iter()
            .map(|row| build_table_row_match_clause(database_kind, definition.as_ref(), row, &table))
            .collect::<Option<Vec<_>>>();
        let Some(where_clauses) = where_clauses else {
            self.status_message = "无法定位当前记录，不能直接删除".into();
            return;
        };
        let sql = format!(
            "DELETE FROM {}\nWHERE {}",
            qualified_table_name(database_kind, &table),
            where_clauses
                .iter()
                .map(|clause| format!("({clause})"))
                .collect::<Vec<_>>()
                .join("\n   OR ")
        );
        let success_message = if row_indices.len() > 1 {
            format!("删除 {} 条记录成功", row_indices.len())
        } else {
            "删除记录成功".into()
        };
        if self.execute_active_table_mutation(sql, success_message) {
            if let Some(WorkspaceTab::Table(tab)) = self.tabs.get_mut(self.active_tab) {
                tab.selected_preview_row = None;
                tab.selected_preview_rows.clear();
                tab.selection_anchor_row = None;
                tab.editing_cell = None;
            }
            self.refresh_active_table_preview(false);
        }
    }

    fn request_delete_active_table_rows(&mut self, mut row_indices: Vec<usize>) {
        let Some(WorkspaceTab::Table(tab)) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.table.is_view {
            self.status_message = "视图暂不支持删除记录".into();
            return;
        }
        row_indices.sort_unstable();
        row_indices.dedup();
        if row_indices.is_empty() {
            self.status_message = "请先选择要删除的记录".into();
            return;
        }
        self.pending_delete_confirmation = Some(PendingDeleteConfirmation {
            active_tab: self.active_tab,
            table_name: tab.title.clone(),
            row_indices,
        });
    }

    fn confirm_pending_delete_rows(&mut self) {
        let Some(pending) = self.pending_delete_confirmation.take() else {
            return;
        };
        if self.active_tab != pending.active_tab {
            self.status_message = "删除确认已过期，请重新选择记录".into();
            return;
        }
        self.delete_active_table_rows(&pending.row_indices);
    }

    fn copy_active_table_rows_as_insert(&mut self, ctx: &egui::Context, row_indices: &[usize]) {
        let Some((database_kind, table, definition, rows)) =
            self.active_table_row_contexts(row_indices)
        else {
            return;
        };
        let columns = definition
            .as_ref()
            .map(|item| item.columns.iter().map(|column| column.name.clone()).collect::<Vec<_>>())
            .filter(|columns| !columns.is_empty())
            .unwrap_or_else(|| rows.first().map(|row| row.keys().cloned().collect()).unwrap_or_default());
        let sql = build_insert_sql_for_existing_rows(database_kind, &table, &columns, &rows);
        ctx.copy_text(sql);
        self.status_message = if row_indices.len() > 1 {
            format!("已复制 {} 条记录的 INSERT 语句", row_indices.len())
        } else {
            "已复制为 INSERT 语句".into()
        };
    }

    fn copy_active_table_rows_as_tsv(&mut self, ctx: &egui::Context, row_indices: &[usize]) {
        let Some((_database_kind, _table, definition, rows)) =
            self.active_table_row_contexts(row_indices)
        else {
            return;
        };
        let columns = definition
            .as_ref()
            .map(|item| item.columns.iter().map(|column| column.name.clone()).collect::<Vec<_>>())
            .filter(|cols| !cols.is_empty())
            .unwrap_or_else(|| rows.first().map(|row| row.keys().cloned().collect()).unwrap_or_default());
        let mut lines: Vec<String> = Vec::with_capacity(rows.len());
        for row in &rows {
            let line: Vec<String> = columns
                .iter()
                .map(|col| {
                    row.get(col)
                        .map(|v| v.as_text().unwrap_or_default().to_string())
                        .unwrap_or_default()
                })
                .collect();
            lines.push(line.join("\t"));
        }
        let text = lines.join("\n");
        ctx.copy_text(text);
        self.status_message = if row_indices.len() > 1 {
            format!("已复制 {} 条记录", row_indices.len())
        } else {
            "已复制数据".into()
        };
    }

    fn active_table_row_context(
        &self,
        row_index: usize,
    ) -> Option<(DatabaseKind, TableRef, Option<TableDefinition>, BTreeMap<String, QueryCellValue>)> {
        let WorkspaceTab::Table(tab) = self.tabs.get(self.active_tab)? else {
            return None;
        };
        let row = tab.preview.as_ref()?.rows.get(row_index)?.clone();
        Some((
            self.database_kind_for_connection(&tab.table.connection_id),
            tab.table.clone(),
            tab.definition.clone(),
            row,
        ))
    }

    fn active_table_row_contexts(
        &self,
        row_indices: &[usize],
    ) -> Option<(
        DatabaseKind,
        TableRef,
        Option<TableDefinition>,
        Vec<BTreeMap<String, QueryCellValue>>,
    )> {
        let WorkspaceTab::Table(tab) = self.tabs.get(self.active_tab)? else {
            return None;
        };
        if row_indices.is_empty() {
            return None;
        }
        let mut rows = Vec::with_capacity(row_indices.len());
        for row_index in row_indices {
            rows.push(tab.preview.as_ref()?.rows.get(*row_index)?.clone());
        }
        Some((
            self.database_kind_for_connection(&tab.table.connection_id),
            tab.table.clone(),
            tab.definition.clone(),
            rows,
        ))
    }

    fn render_query_tab(
        ui: &mut egui::Ui,
        tab: &mut QueryTabState,
        connections: &[ConnectionProfile],
        selected_connection: Option<String>,
        database_cache: &HashMap<String, Vec<String>>,
        services: &AppServices,
    ) -> TabUiAction {
        let mut action = TabUiAction::None;
        let chrome = mac_ui_palette(ui.visuals());
        let selected_connection_label = tab
            .connection_id
            .as_ref()
            .and_then(|id| connections.iter().find(|item| &item.id == id))
            .map(|item| item.name.clone())
            .unwrap_or_else(|| "跟随当前选中连接".into());
        let has_result = tab.result.is_some();
        // 首次打开编辑器高度取默认值
        let editor_height = tab.editor_height.unwrap_or(200.0);
        let mut strip_builder = StripBuilder::new(ui)
            .size(Size::exact(90.0));
        if has_result {
            strip_builder = strip_builder
                .size(Size::exact(editor_height + 12.0))
                .size(Size::exact(8.0))
                .size(Size::remainder());
        } else {
            strip_builder = strip_builder.size(Size::remainder());
        }
        strip_builder.vertical(|mut strip| {
                strip.cell(|ui| {
                    egui::Frame::new()
                        .fill(chrome.toolbar_bg)
                        .stroke(Stroke::new(1.0, chrome.border))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
                            // 第一行：执行按钮
                            ui.horizontal(|ui| {
                                if toolbar_button(ui, "执行全部", ToolbarButtonKind::Accent).clicked()
                                {
                                    action = TabUiAction::ExecuteQuery(ExecuteMode::Whole);
                                }
                                if toolbar_button(ui, "执行选中SQL", ToolbarButtonKind::Primary).clicked()
                                {
                                    let selected = tab.cursor_range
                                        .and_then(|r| if !r.is_empty() { Some(r.slice_str(&tab.sql).to_string()) } else { None });
                                    action = TabUiAction::ExecuteQuery(ExecuteMode::Selection(selected));
                                }
                                if toolbar_button(ui, "保存查询", ToolbarButtonKind::Secondary).clicked()
                                {
                                    if let Some(connection_id) =
                                        tab.connection_id.clone().or_else(|| selected_connection.clone())
                                    {
                                        action = TabUiAction::OpenSaveQueryDialog(connection_id);
                                    } else {
                                        tab.messages.push("请先选择一个连接后再保存查询".into());
                                        tab.active_bottom_tab = QueryBottomTab::Messages;
                                    }
                                }
                                if toolbar_button(ui, "格式化", ToolbarButtonKind::Subtle).clicked() {
                                    tab.sql = simple_format_sql(&tab.sql);
                                    tab.messages.push("已格式化 SQL".into());
                                    tab.active_bottom_tab = QueryBottomTab::Messages;
                                }
                            });
                            ui.add_space(10.0);
                            // 第二行：连接 + 数据库
                            ui.horizontal(|ui| {
                                ui.set_min_width(ui.available_width());
                                ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
                                // 连接
                                ui.label(RichText::new("连接").color(chrome.weak_text));
                                let connection_combo = egui::ComboBox::from_id_salt(format!(
                                    "query-connection-{}",
                                    tab.id
                                ))
                                .width(200.0)
                                .selected_text(selected_connection_label)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut tab.connection_id,
                                        None,
                                        "跟随当前选中连接",
                                    );
                                    for connection in connections {
                                        ui.selectable_value(
                                            &mut tab.connection_id,
                                            Some(connection.id.clone()),
                                            &connection.name,
                                        );
                                    }
                                });
                                // 切换连接时重新加载已保存查询
                                if connection_combo.response.changed() {
                                    if let Some(ref cid) = tab.connection_id {
                                        let (history, saved_queries) = load_query_library(
                                            services,
                                            cid,
                                        );
                                        tab.history = history;
                                        tab.saved_queries = saved_queries;
                                    } else {
                                        tab.history.clear();
                                        tab.saved_queries.clear();
                                    }
                                }
                                ui.add_space(24.0);

                                // 数据库
                                ui.label(RichText::new("数据库").color(chrome.weak_text));
                                let effective_connection_id = tab
                                    .connection_id
                                    .clone()
                                    .or_else(|| selected_connection.clone());
                                let databases = effective_connection_id
                                    .as_ref()
                                    .and_then(|cid| database_cache.get(cid));
                                let db_label = tab
                                    .database
                                    .as_deref()
                                    .unwrap_or("-- 数据库 --");
                                let db_combo = egui::ComboBox::from_id_salt(format!(
                                    "query-database-{}",
                                    tab.id
                                ))
                                .width(180.0)
                                .selected_text(db_label)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut tab.database,
                                        None,
                                        "-- 数据库 --",
                                    );
                                    if let Some(dbs) = databases {
                                        for db in dbs {
                                            ui.selectable_value(
                                                &mut tab.database,
                                                Some(db.clone()),
                                                db,
                                            );
                                        }
                                    }
                                });
                                // 响应数据库选择变化
                                if db_combo.response.changed() {
                                    tab.messages.push(format!(
                                        "已选择数据库: {}",
                                        tab.database.as_deref().unwrap_or("(无)")
                                    ));
                                }
                            });
                        });
                });

                strip.cell(|ui| {
                    let palette = editor_palette(ui.visuals());
                    egui::Frame::new()
                        .fill(palette.panel_bg)
                        .stroke(Stroke::new(1.0, chrome.soft_border))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::same(0))
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(palette.editor_bg)
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::same(0))
                                .show(ui, |ui| {
                                    let editor_inner_height = ui.available_height();
                                    if tab.saved_queries_panel_visible {
                                        // 左侧可折叠面板 + 右侧编辑器
                                        StripBuilder::new(ui)
                                            .size(Size::exact(220.0))
                                            .size(Size::exact(1.0)) // separator
                                            .size(Size::remainder())
                                            .horizontal(|mut h_strip| {
                                                // 左侧面板：已保存查询
                                                h_strip.cell(|ui| {
                                                    render_saved_queries_panel(
                                                        ui,
                                                        tab,
                                                        chrome,
                                                        &mut action,
                                                    );
                                                });
                                                // 分隔线
                                                h_strip.cell(|ui| {
                                                    let rect = ui.max_rect();
                                                    ui.painter().line_segment(
                                                        [
                                                            egui::pos2(rect.center().x, rect.top() + 4.0),
                                                            egui::pos2(rect.center().x, rect.bottom() - 4.0),
                                                        ],
                                                        Stroke::new(1.0, chrome.soft_border),
                                                    );
                                                    ui.allocate_space(rect.size());
                                                });
                                                // 右侧：编辑器
                                                h_strip.cell(|ui| {
                                                    render_query_editor(
                                                        ui,
                                                        tab,
                                                        &palette,
                                                        editor_inner_height,
                                                        &mut action,
                                                    );
                                                });
                                            });
                                    } else {
                                        // 面板折叠时，在编辑器左侧显示展开按钮
                                        StripBuilder::new(ui)
                                            .size(Size::exact(24.0))
                                            .size(Size::remainder())
                                            .horizontal(|mut h_strip| {
                                                h_strip.cell(|ui| {
                                                    let palette = mac_ui_palette(ui.visuals());
                                                    let (id, rect) = ui.allocate_space(egui::vec2(24.0, ui.available_height()));
                                                    let sense = egui::Sense::click();
                                                    let response = ui.interact(rect, id, sense);
                                                    if response.hovered() {
                                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                                    }
                                                    if response.clicked() {
                                                        tab.saved_queries_panel_visible = true;
                                                    }
                                                    let galley = ui.painter().layout_no_wrap(
                                                        "▶".to_string(),
                                                        FontId::new(10.0, FontFamily::Proportional),
                                                        palette.weak_text,
                                                    );
                                                    let text_pos = egui::pos2(
                                                        rect.center().x - galley.size().x / 2.0,
                                                        rect.top() + 10.0,
                                                    );
                                                    ui.painter().with_clip_rect(rect);
                                                    ui.painter().galley(text_pos, galley, palette.weak_text);
                                                });
                                                h_strip.cell(|ui| {
                                                    render_query_editor(
                                                        ui,
                                                        tab,
                                                        &palette,
                                                        editor_inner_height,
                                                        &mut action,
                                                    );
                                                });
                                            });
                                    }
                                });
                        });
                    }); // end strip.cell (editor)

                // 拖拽把手（作为单独的 strip cell）
                if has_result {
                    strip.cell(|ui| {
                        let handle_id = egui::Id::from(format!("query-split-handle-{}", tab.id));
                        let (handle_rect, handle_response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 8.0),
                            egui::Sense::drag(),
                        );
                        let _ = handle_id;
                        handle_response.widget_info(|| egui::WidgetInfo::drag_value(false, 0.0));
                        if handle_response.hovered() || handle_response.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                        }
                        // 拖拽时更新编辑器高度
                        if handle_response.dragged() {
                            let delta = handle_response.drag_delta().y;
                            let new = (editor_height + delta).max(100.0);
                            tab.editor_height = Some(new);
                            ui.ctx().request_repaint();
                        }
                        // 可视把手线
                        let line_y = handle_rect.center().y;
                        ui.painter().line_segment(
                            [
                                egui::pos2(handle_rect.left() + ui.available_width() * 0.35, line_y),
                                egui::pos2(handle_rect.right() - ui.available_width() * 0.35, line_y),
                            ],
                            Stroke::new(1.5, chrome.soft_border),
                        );
                    });
                }

                if has_result {
                    strip.cell(|ui| {
                        egui::Frame::new()
                            .fill(chrome.card_bg)
                            .stroke(Stroke::new(1.0, chrome.border))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(8, 8))
                            .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for pane in [
                                    QueryBottomTab::Results,
                                    QueryBottomTab::Messages,
                                    QueryBottomTab::History,
                                ] {
                                    let label = pane.label();
                                    if segment_button(ui, label, tab.active_bottom_tab == pane).clicked() {
                                        tab.active_bottom_tab = pane;
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let summary = match tab.active_bottom_tab {
                                            QueryBottomTab::Results => tab
                                                .result
                                                .as_ref()
                                                .map(|result| {
                                                    let base = format!(
                                                        "{} 列 / {} 行 / {} ms",
                                                        result.columns.len(),
                                                        result.rows.len(),
                                                        result.elapsed_ms
                                                    );
                                                    if tab.multi_results.len() > 1 {
                                                        format!(
                                                            "结果 {}/{} — {}",
                                                            tab.selected_result_index + 1,
                                                            tab.multi_results.len(),
                                                            base
                                                        )
                                                    } else {
                                                        base
                                                    }
                                                })
                                                .unwrap_or_else(|| "等待执行 SQL".into()),
                                            QueryBottomTab::Messages => format!(
                                                "{} 条消息",
                                                tab.messages.len() + usize::from(tab.error.is_some())
                                            ),
                                            QueryBottomTab::History => {
                                                format!("{} 条历史", tab.history.len())
                                            }
                                        };
                                        ui.small(RichText::new(summary).color(chrome.weak_text));
                                    },
                                );
                            });
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(8.0);

                            match tab.active_bottom_tab {
                                QueryBottomTab::Results => {
                                    // 多语句结果切换器
                                    if tab.multi_results.len() > 1 {
                                        ui.horizontal(|ui| {
                                            ui.small(RichText::new("结果:").color(chrome.weak_text));
                                            for (index, _) in tab.multi_results.iter().enumerate() {
                                                let label = format!("结果{}", index + 1);
                                                let is_selected = index == tab.selected_result_index;
                                                if segment_button(ui, &label, is_selected).clicked() {
                                                    tab.selected_result_index = index;
                                                    if let Some(mut selected) =
                                                        tab.multi_results.get(index).cloned()
                                                    {
                                                        apply_saved_table_sort(
                                                            &mut selected,
                                                            &mut tab.result_sort,
                                                        );
                                                        tab.result = Some(selected);
                                                    }
                                                }
                                            }
                                        });
                                        ui.add_space(8.0);
                                        ui.separator();
                                        ui.add_space(8.0);
                                    }
                                    if let Some(result) = &mut tab.result {
                                        let _ =
                                            render_result_table(ui, result, &mut tab.result_sort, false, &mut tab.selected_columns);
                                    } else {
                                        render_query_empty_state(
                                            ui,
                                            "暂无查询结果",
                                            "执行一条查询语句后，结果会显示在这里",
                                        );
                                    }
                                }
                                QueryBottomTab::Messages => {
                                    egui::ScrollArea::vertical()
                                        .id_salt(format!("query-messages-{}", tab.id))
                                        .show(ui, |ui| {
                                            if let Some(error) = &tab.error {
                                                egui::Frame::new()
                                                    .fill(Color32::from_rgba_premultiplied(
                                                        chrome.danger.r(),
                                                        chrome.danger.g(),
                                                        chrome.danger.b(),
                                                        22,
                                                    ))
                                                    .stroke(Stroke::new(1.0, chrome.danger))
                                                    .corner_radius(6.0)
                                                    .inner_margin(egui::Margin::symmetric(10, 8))
                                                    .show(ui, |ui| {
                                                        ui.colored_label(chrome.danger, error);
                                                    });
                                                ui.separator();
                                            }
                                            if let Some(sql) = &tab.last_executed_sql {
                                                ui.small(
                                                    RichText::new("最近执行 SQL")
                                                        .strong()
                                                        .color(chrome.weak_text),
                                                );
                                                ui.add_space(4.0);
                                                let mut sql_text = sql.clone();
                                                egui::Frame::new()
                                                    .fill(chrome.search_bg)
                                                    .stroke(Stroke::new(1.0, chrome.soft_border))
                                                    .corner_radius(6.0)
                                                    .inner_margin(egui::Margin::symmetric(8, 6))
                                                    .show(ui, |ui| {
                                                        egui::ScrollArea::horizontal()
                                                            .id_salt(format!(
                                                                "query-last-sql-{}",
                                                                tab.id
                                                            ))
                                                            .auto_shrink([false, false])
                                                            .show(ui, |ui| {
                                                                ui.add(
                                                                    TextEdit::singleline(&mut sql_text)
                                                                        .font(egui::TextStyle::Monospace)
                                                                        .desired_width(f32::INFINITY)
                                                                        .interactive(false)
                                                                        .frame(false),
                                                                );
                                                            });
                                                    });
                                                ui.separator();
                                            }
                                            if tab.messages.is_empty() {
                                                render_query_empty_state(
                                                    ui,
                                                    "暂无消息",
                                                    "格式化、执行或清空操作的反馈会显示在这里",
                                                );
                                            } else {
                                                for message in tab.messages.iter().rev() {
                                                    egui::Frame::new()
                                                        .fill(chrome.search_bg)
                                                        .stroke(Stroke::new(1.0, chrome.soft_border))
                                                        .corner_radius(6.0)
                                                        .inner_margin(egui::Margin::symmetric(8, 6))
                                                        .show(ui, |ui| {
                                                            ui.label(message);
                                                        });
                                                    ui.add_space(4.0);
                                                }
                                            }
                                        });
                                }
                                QueryBottomTab::History => {
                                    egui::ScrollArea::vertical()
                                        .id_salt(format!("query-history-{}", tab.id))
                                        .show(ui, |ui| {
                                            if tab.saved_queries.is_empty() && tab.history.is_empty() {
                                                render_query_empty_state(
                                                    ui,
                                                    "暂无查询记录",
                                                    "保存的查询和执行历史会显示在这里，方便再次打开",
                                                );
                                            } else {
                                                ui.small(
                                                    RichText::new("已保存查询")
                                                        .strong()
                                                        .color(chrome.weak_text),
                                                );
                                                ui.add_space(6.0);
                                                if tab.saved_queries.is_empty() {
                                                    ui.small(
                                                        RichText::new("还没有保存过查询")
                                                            .color(chrome.weak_text),
                                                    );
                                                } else {
                                                    for entry in &tab.saved_queries {
                                                        let response = ui.add_sized(
                                                            [ui.available_width(), 42.0],
                                                            egui::Button::new(
                                                                RichText::new(format!(
                                                                    "{}  ·  {}",
                                                                    truncate_ui_label(&entry.title, 30),
                                                                    truncate_ui_label(
                                                                        &compact_query_preview(
                                                                            &entry.sql_text
                                                                        ),
                                                                        40,
                                                                    )
                                                                ))
                                                                .size(12.0)
                                                                .color(chrome.text),
                                                            )
                                                            .fill(chrome.search_bg)
                                                            .stroke(Stroke::new(1.0, chrome.soft_border))
                                                            .corner_radius(6.0),
                                                        );
                                                        if response.clicked() {
                                                            tab.sql = entry.sql_text.clone();
                                                            tab.connection_id =
                                                                Some(entry.connection_id.clone());
                                                            tab.database = entry.database.clone();
                                                        }
                                                        ui.add_space(4.0);
                                                    }
                                                }

                                                ui.add_space(8.0);
                                                ui.separator();
                                                ui.add_space(8.0);
                                                ui.small(
                                                    RichText::new("执行历史")
                                                        .strong()
                                                        .color(chrome.weak_text),
                                                );
                                                ui.add_space(6.0);
                                                if tab.history.is_empty() {
                                                    ui.small(
                                                        RichText::new("还没有执行过查询")
                                                            .color(chrome.weak_text),
                                                    );
                                                } else {
                                                    for item in &tab.history {
                                                        let preview = compact_query_preview(item);
                                                        let response = ui.add_sized(
                                                            [ui.available_width(), 28.0],
                                                            egui::Button::new(
                                                                RichText::new(truncate_ui_label(
                                                                    &preview,
                                                                    60,
                                                                ))
                                                                .size(12.0)
                                                                .color(chrome.text),
                                                            )
                                                            .fill(chrome.search_bg)
                                                            .stroke(Stroke::new(1.0, chrome.soft_border))
                                                            .corner_radius(6.0),
                                                        );
                                                        if response.clicked() {
                                                            tab.sql = item.clone();
                                                        }
                                                        ui.add_space(4.0);
                                                    }
                                                }
                                            }
                                        });
                                }
                            }
                        });
                });
                }
            });
        action
    }

    fn render_table_tab(ui: &mut egui::Ui, tab: &mut TableTabState) -> TabUiAction {
        let palette = mac_ui_palette(ui.visuals());
        let show_table_loading = |ui: &mut egui::Ui, label: &str| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.add(egui::Spinner::new().size(40.0));
                ui.add_space(12.0);
                ui.label(RichText::new(label).color(palette.weak_text));
            });
        };
        let mut action = TabUiAction::None;
        StripBuilder::new(ui)
            .size(Size::exact(38.0))
            .size(Size::remainder())
            .size(Size::exact(26.0))
            .vertical(|mut strip| {
                strip.cell(|ui| {
                    egui::Frame::new()
                        .fill(palette.toolbar_bg)
                        .stroke(Stroke::new(1.0, palette.border))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(tab.table.label())
                                        .strong()
                                        .color(palette.text),
                                );
                                ui.separator();
                                for mode in [
                                    TableViewMode::Data,
                                    TableViewMode::Structure,
                                    TableViewMode::Definition,
                                ] {
                                    if segment_button(ui, mode.label(), tab.active_view == mode).clicked() {
                                        tab.active_view = mode;
                                    }
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if mini_button(ui, "新建查询", MiniButtonKind::Accent).clicked() {
                                            action = TabUiAction::NewQueryFromTable {
                                                connection_id: tab.table.connection_id.clone(),
                                                database: tab.table.database.clone(),
                                                schema: tab.table.schema.clone(),
                                                table: tab.table.table.clone(),
                                            };
                                        }
                                        ui.separator();
                                    },
                                );
                            });
                        });
                });

                strip.cell(|ui| {
                    egui::Frame::new()
                        .fill(palette.card_bg)
                        .stroke(Stroke::new(1.0, palette.border))
                        .inner_margin(egui::Margin::symmetric(8, 8))
                        .show(ui, |ui| {
                            if let Some(error) = &tab.error {
                                ui.colored_label(palette.danger, error);
                                ui.separator();
                            }

                            match tab.active_view {
                                TableViewMode::Data => {
                                    if tab.preview.is_some() {
                                        let live_preview_sql = build_table_preview_display_sql(
                                            tab.database_kind,
                                            &tab.table,
                                            &tab.preview_filter,
                                            &tab.preview_sort,
                                            if tab.preview_limit_enabled {
                                                Some(tab.preview_page_size.max(1))
                                            } else {
                                                None
                                            },
                                        );
                                        ui.horizontal(|ui| {
                                            if mini_button(ui, "刷新", MiniButtonKind::Subtle).clicked() {
                                                action =
                                                    TabUiAction::RefreshActiveTable { reload_definition: true };
                                            }
                                            if mini_button(ui, "新增", MiniButtonKind::Subtle).clicked() {
                                                let columns = table_editable_columns(tab);
                                                tab.pending_insert_row =
                                                    Some(create_empty_insert_row(&columns));
                                                tab.selected_preview_row = None;
                                                tab.selected_preview_rows.clear();
                                                tab.selection_anchor_row = None;
                                                tab.editing_cell = columns.first().map(|column| {
                                                    TableCellEditState {
                                                        target: TableEditTarget::PendingInsert,
                                                        column: column.clone(),
                                                        value: String::new(),
                                                        is_null: false,
                                                        focus_requested: true,
                                                    }
                                                });
                                            }
                                            let filter_active = tab.show_preview_filter
                                                || table_filter_summary(&tab.preview_filter)
                                                    .is_some();
                                            let filter_kind = if filter_active {
                                                MiniButtonKind::Accent
                                            } else {
                                                MiniButtonKind::Subtle
                                            };
                                            if mini_button(ui, "筛选", filter_kind).clicked() {
                                                tab.show_preview_filter = !tab.show_preview_filter;
                                            }
                                            if mini_button(ui, "导出", MiniButtonKind::Subtle).clicked() {
                                                action = TabUiAction::ExportActiveResult;
                                            }
                                            if tab.pending_insert_row.is_some() {
                                                ui.separator();
                                                if mini_button(ui, "保存新增", MiniButtonKind::Accent).clicked()
                                                {
                                                    action = TabUiAction::SavePendingInsertRow;
                                                }
                                                if mini_button(ui, "取消新增", MiniButtonKind::Danger)
                                                    .clicked()
                                                {
                                                    tab.pending_insert_row = None;
                                                    tab.editing_cell = None;
                                                }
                                            }
                                            ui.separator();
                                            ui.small(
                                                RichText::new("结果集")
                                                    .strong()
                                                    .color(palette.weak_text),
                                            );
                                            if let Some(summary) =
                                                table_filter_summary(&tab.preview_filter)
                                            {
                                                ui.small(
                                                    RichText::new(format!("筛选: {summary}"))
                                                        .color(palette.selection_text),
                                                );
                                            }
                                            let preview_rows = tab
                                                .preview
                                                .as_ref()
                                                .map(|preview| preview.rows.len())
                                                .unwrap_or(0);
                                            ui.small(
                                                RichText::new(format!("共 {} 行", preview_rows))
                                                    .color(palette.weak_text),
                                            );
                                        });
                                        let available_columns = table_filter_columns(tab);
                                        ensure_table_filter_column(
                                            &mut tab.preview_filter,
                                            &available_columns,
                                        );
                                        if tab.show_preview_filter
                                            || table_filter_summary(&tab.preview_filter).is_some()
                                        {
                                            ui.add_space(6.0);
                                            egui::Frame::new()
                                                .fill(palette.search_bg)
                                                .stroke(Stroke::new(1.0, palette.soft_border))
                                                .corner_radius(6.0)
                                                .inner_margin(egui::Margin::symmetric(8, 8))
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.small(
                                                            RichText::new("筛选条件")
                                                                .strong()
                                                                .color(palette.text),
                                                        );
                                                        if let Some(summary) =
                                                            table_filter_summary(&tab.preview_filter)
                                                        {
                                                            ui.add_space(6.0);
                                                            ui.small(
                                                                RichText::new(summary)
                                                                    .color(palette.selection_text),
                                                            );
                                                        }
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                if mini_button(
                                                                    ui,
                                                                    "清空",
                                                                    MiniButtonKind::Subtle,
                                                                )
                                                                .clicked()
                                                                {
                                                                    tab.preview_filter =
                                                                        TableFilterState::default();
                                                                    ensure_table_filter_column(
                                                                        &mut tab.preview_filter,
                                                                        &available_columns,
                                                                    );
                                                                    action =
                                                                        TabUiAction::RefreshActiveTable {
                                                                            reload_definition: false,
                                                                        };
                                                                }
                                                                if mini_button(
                                                                    ui,
                                                                    "应用",
                                                                    MiniButtonKind::Subtle,
                                                                )
                                                                .clicked()
                                                                {
                                                                    action =
                                                                        TabUiAction::RefreshActiveTable {
                                                                            reload_definition: false,
                                                                        };
                                                                }
                                                            },
                                                        );
                                                    });
                                                    ui.add_space(8.0);
                                                    let mut pending_remove_clause = None;
                                                    let mut add_clause = false;
                                                    let clause_count = tab.preview_filter.clauses.len();
                                                    for (index, clause) in
                                                        tab.preview_filter.clauses.iter_mut().enumerate()
                                                    {
                                                        if index > 0 {
                                                            ui.add_space(6.0);
                                                        }
                                                        ui.horizontal_wrapped(|ui| {
                                                            if index == 0 {
                                                                ui.add_sized(
                                                                    [54.0, 26.0],
                                                                    egui::Label::new(
                                                                        RichText::new("首个")
                                                                            .color(palette.weak_text),
                                                                    ),
                                                                );
                                                            } else {
                                                                egui::ComboBox::from_id_salt(format!(
                                                                    "table-filter-joiner-{}-{}",
                                                                    tab.title, index
                                                                ))
                                                                .width(72.0)
                                                                .selected_text(clause.joiner.label())
                                                                .show_ui(ui, |ui| {
                                                                    for joiner in TableFilterJoiner::ALL
                                                                    {
                                                                        ui.selectable_value(
                                                                            &mut clause.joiner,
                                                                            joiner,
                                                                            joiner.label(),
                                                                        );
                                                                    }
                                                                });
                                                            }
                                                            egui::ComboBox::from_id_salt(format!(
                                                                "table-filter-column-{}-{}",
                                                                tab.title, index
                                                            ))
                                                            .width(140.0)
                                                            .selected_text(
                                                                clause
                                                                    .column
                                                                    .clone()
                                                                    .unwrap_or_else(|| "选择列".into()),
                                                            )
                                                            .show_ui(ui, |ui| {
                                                                for column in &available_columns {
                                                                    ui.selectable_value(
                                                                        &mut clause.column,
                                                                        Some(column.clone()),
                                                                        column,
                                                                    );
                                                                }
                                                            });
                                                            egui::ComboBox::from_id_salt(format!(
                                                                "table-filter-operator-{}-{}",
                                                                tab.title, index
                                                            ))
                                                            .width(110.0)
                                                            .selected_text(clause.operator.label())
                                                            .show_ui(ui, |ui| {
                                                                for operator in TableFilterOperator::ALL {
                                                                    ui.selectable_value(
                                                                        &mut clause.operator,
                                                                        operator,
                                                                        operator.label(),
                                                                    );
                                                                }
                                                            });
                                                            if clause.operator == TableFilterOperator::Custom {
                                                                ui.add_sized(
                                                                    [360.0, 26.0],
                                                                    TextEdit::singleline(&mut clause.value)
                                                                        .hint_text("输入原始 SQL 条件"),
                                                                );
                                                            } else if clause.operator.uses_secondary_value() {
                                                                ui.add_sized(
                                                                    [150.0, 26.0],
                                                                    TextEdit::singleline(&mut clause.value)
                                                                        .hint_text("起始值"),
                                                                );
                                                                ui.small(
                                                                    RichText::new("到")
                                                                        .color(palette.weak_text),
                                                                );
                                                                ui.add_sized(
                                                                    [150.0, 26.0],
                                                                    TextEdit::singleline(
                                                                        &mut clause.second_value,
                                                                    )
                                                                    .hint_text("结束值"),
                                                                );
                                                            } else if clause.operator.uses_primary_value() {
                                                                ui.add_sized(
                                                                    [240.0, 26.0],
                                                                    TextEdit::singleline(&mut clause.value)
                                                                        .hint_text(clause.operator.value_hint()),
                                                                );
                                                            } else {
                                                                ui.small(
                                                                    RichText::new("当前条件无需输入值")
                                                                        .color(palette.weak_text),
                                                                );
                                                            }
                                                            if mini_button(
                                                                ui,
                                                                if index + 1 == clause_count {
                                                                    "新增条件"
                                                                } else {
                                                                    "+"
                                                                },
                                                                MiniButtonKind::Subtle,
                                                            )
                                                            .clicked()
                                                            {
                                                                add_clause = true;
                                                            }
                                                            if clause_count > 1
                                                                && mini_button(
                                                                    ui,
                                                                    "删除",
                                                                    MiniButtonKind::Subtle,
                                                                )
                                                                .clicked()
                                                            {
                                                                pending_remove_clause = Some(index);
                                                            }
                                                        });
                                                    }
                                                    if add_clause {
                                                        let mut clause = tab
                                                            .preview_filter
                                                            .clauses
                                                            .last()
                                                            .cloned()
                                                            .unwrap_or_default();
                                                        clause.joiner = TableFilterJoiner::And;
                                                        if clause.column.is_none() {
                                                            clause.column = available_columns.first().cloned();
                                                        }
                                                        clause.value.clear();
                                                        clause.second_value.clear();
                                                        tab.preview_filter.clauses.push(clause);
                                                    }
                                                    if let Some(index) = pending_remove_clause {
                                                        tab.preview_filter.clauses.remove(index);
                                                        if tab.preview_filter.clauses.is_empty() {
                                                            tab.preview_filter
                                                                .clauses
                                                                .push(TableFilterClause::default());
                                                        }
                                                        ensure_table_filter_column(
                                                            &mut tab.preview_filter,
                                                            &available_columns,
                                                        );
                                                    }
                                                    ui.add_space(8.0);
                                                    ui.horizontal(|ui| {
                                                        ui.small(
                                                            RichText::new("预览 SQL")
                                                                .strong()
                                                                .color(palette.weak_text),
                                                        );
                                                        if tab
                                                            .last_preview_sql
                                                            .as_ref()
                                                            .is_some_and(|sql| sql != &live_preview_sql)
                                                        {
                                                            ui.small(
                                                                RichText::new("未应用")
                                                                    .color(palette.selection_text),
                                                            );
                                                        }
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                if mini_button(
                                                                    ui,
                                                                    "复制",
                                                                    MiniButtonKind::Subtle,
                                                                )
                                                                .clicked()
                                                                {
                                                                    action =
                                                                        TabUiAction::CopyTextToClipboard {
                                                                            text: live_preview_sql
                                                                                .clone(),
                                                                            status_message:
                                                                                "已复制预览 SQL"
                                                                                    .into(),
                                                                        };
                                                                }
                                                            },
                                                        );
                                                    });
                                                    ui.add_space(4.0);
                                                    egui::Frame::new()
                                                        .fill(palette.card_bg)
                                                        .stroke(Stroke::new(
                                                            1.0,
                                                            palette.soft_border,
                                                        ))
                                                        .corner_radius(5.0)
                                                        .inner_margin(egui::Margin::same(8))
                                                        .show(ui, |ui| {
                                                            let mut sql_text = live_preview_sql.clone();
                                                            egui::ScrollArea::horizontal()
                                                                .id_salt(format!(
                                                                    "table-preview-sql-{}",
                                                                    tab.title
                                                                ))
                                                                .max_height(28.0)
                                                                .auto_shrink([false, false])
                                                                .show(ui, |ui| {
                                                                    ui.add(
                                                                        TextEdit::singleline(
                                                                            &mut sql_text,
                                                                        )
                                                                        .font(
                                                                            egui::TextStyle::Monospace,
                                                                        )
                                                                        .desired_width(
                                                                            f32::INFINITY,
                                                                        )
                                                                        .interactive(false)
                                                                        .frame(false),
                                                                    );
                                                                });
                                                        });
                                                });
                                        }
                                        if let Some(preview) = &mut tab.preview {
                                            ui.data_mut(|data| {
                                                data.insert_temp(
                                                    egui::Id::new("table-preview-meta"),
                                                    (
                                                        preview.columns.len(),
                                                        preview.rows.len(),
                                                        preview.elapsed_ms,
                                                    ),
                                                );
                                            });
                                            ui.add_space(6.0);
                                            ui.separator();
                                            ui.add_space(6.0);
                                            let table_action = render_editable_table(ui, tab);
                                            if !matches!(table_action, TabUiAction::None) {
                                                action = table_action;
                                            }
                                        }
                                    } else if tab.error.is_none() {
                                        show_table_loading(ui, "正在加载表数据...");
                                        ui.data_mut(|data| {
                                            data.insert_temp(
                                                egui::Id::new("table-preview-meta"),
                                                (0usize, 0usize, 0u128),
                                            );
                                        });
                                    } else {
                                        ui.label("暂无预览数据");
                                        ui.data_mut(|data| {
                                            data.insert_temp(
                                                egui::Id::new("table-preview-meta"),
                                                (0usize, 0usize, 0u128),
                                            );
                                        });
                                    }
                                }
                                TableViewMode::Structure => {
                                    if let Some(definition) = &tab.definition {
                                        render_table_structure_grid(ui, definition);
                                    } else if tab.error.is_none() {
                                        show_table_loading(ui, "正在加载表结构...");
                                    } else {
                                        ui.label("暂无结构信息");
                                    }
                                }
                                TableViewMode::Definition => {
                                    if let Some(definition) = &tab.definition {
                                        if let Some(create_sql) = &definition.create_sql {
                                            render_definition_sql_view(
                                                ui,
                                                &tab.title,
                                                create_sql,
                                            );
                                        } else {
                                            ui.label("当前对象没有可展示的 DDL");
                                        }
                                    } else if tab.error.is_none() {
                                        show_table_loading(ui, "正在加载 DDL...");
                                    } else {
                                        ui.label("暂无 DDL");
                                    }
                                }
                            }
                        });
                });

                strip.cell(|ui| {
                    let row_count = tab.preview.as_ref().map(|item| item.rows.len()).unwrap_or(0);
                    let page_size = tab.preview_page_size.max(1);
                    let column_count = tab
                        .definition
                        .as_ref()
                        .map(|item| item.columns.len())
                        .or_else(|| tab.preview.as_ref().map(|item| item.columns.len()))
                        .unwrap_or(0);
                    let (result_column_count, result_row_count, result_elapsed_ms) = ui
                        .data(|data| {
                            data.get_temp::<(usize, usize, u128)>(egui::Id::new("table-preview-meta"))
                        })
                        .unwrap_or((0, 0, 0));
                    ui.horizontal(|ui| {
                        let displayed_end = if row_count == 0 {
                            0
                        } else if tab.preview_limit_enabled {
                            row_count.min(page_size as usize)
                        } else {
                            row_count
                        };
                        let mut footer_refresh_requested = false;
                        let _ = mini_button(ui, "<<", MiniButtonKind::Subtle);
                        let _ = mini_button(ui, "<", MiniButtonKind::Subtle);
                        ui.small(RichText::new("第 1 页").color(palette.weak_text));
                        ui.separator();
                        let _ = mini_button(ui, ">", MiniButtonKind::Subtle);
                        let _ = mini_button(ui, ">>", MiniButtonKind::Subtle);
                        ui.separator();
                        ui.small(RichText::new(format!("记录 {row_count}")).color(palette.weak_text));
                        ui.separator();
                        ui.small(RichText::new(format!("字段 {column_count}")).color(palette.weak_text));
                        ui.separator();
                        ui.small(
                            RichText::new(format!("列 {result_column_count}")).color(palette.weak_text),
                        );
                        ui.separator();
                        ui.small(
                            RichText::new(format!("行 {result_row_count}")).color(palette.weak_text),
                        );
                        ui.separator();
                        ui.small(
                            RichText::new(format!("耗时 {result_elapsed_ms} ms")).color(palette.weak_text),
                        );
                        ui.separator();
                        let limit_changed = ui
                            .checkbox(&mut tab.preview_limit_enabled, "限制")
                            .changed();
                        let page_size_changed = ui
                            .add_enabled_ui(tab.preview_limit_enabled, |ui| {
                                ui.add_sized(
                                    [64.0, 22.0],
                                    egui::DragValue::new(&mut tab.preview_page_size)
                                        .range(1..=100_000)
                                        .speed(1.0),
                                )
                            })
                            .inner
                            .changed();
                        ui.add_space(2.0);
                        ui.small(RichText::new("条记录（每页）").color(palette.weak_text));
                        ui.add_space(2.0);
                        ui.menu_button(RichText::new("⚙").size(13.0).color(palette.weak_text), |ui| {
                            ui.set_min_width(140.0);
                            if ui.button("重置为 1000").clicked() {
                                tab.preview_limit_enabled = true;
                                tab.preview_page_size = 1000;
                                footer_refresh_requested = true;
                                ui.close();
                            }
                            if ui
                                .button(if tab.preview_limit_enabled {
                                    "关闭限制"
                                } else {
                                    "开启限制"
                                })
                                .clicked()
                            {
                                tab.preview_limit_enabled = !tab.preview_limit_enabled;
                                footer_refresh_requested = true;
                                ui.close();
                            }
                            if ui.button("刷新数据").clicked() {
                                footer_refresh_requested = true;
                                ui.close();
                            }
                        });
                        ui.separator();
                        ui.small(
                            RichText::new(format!(
                                "范围 1-{} / {}",
                                displayed_end,
                                row_count
                            ))
                                .color(palette.weak_text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.small(
                                RichText::new(
                                    tab.last_preview_sql
                                        .clone()
                                        .unwrap_or_else(|| format!("SELECT * FROM {}", tab.table.label())),
                                )
                                .color(palette.weak_text),
                            );
                        });
                        if limit_changed || page_size_changed || footer_refresh_requested {
                            action = TabUiAction::RefreshActiveTable {
                                reload_definition: false,
                            };
                        }
                    });
                });
            });
        action
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        let palette = mac_ui_palette(ui.visuals());
        ui.horizontal_wrapped(|ui| {
            let color = match self.status_level {
                StatusLevel::Pending => palette.selection_text,
                StatusLevel::Success => Color32::from_rgb(0x34, 0xC7, 0x59),
                StatusLevel::Error => palette.danger,
                StatusLevel::Normal => palette.weak_text,
            };
            ui.label(RichText::new(&self.status_message).color(color));
            if let Some(connection_id) = &self.selected_connection {
                let conn_name = self.connection_name(connection_id);
                ui.separator();
                ui.label(RichText::new(format!("当前连接: {conn_name}")).color(palette.weak_text));
            }
        });
    }

    fn render_connection_dialog(&mut self, ctx: &egui::Context) {
        if !self.is_connection_dialog_open {
            return;
        }
        let mut open = self.is_connection_dialog_open;
        let mut should_close = false;
        let title = if self.editing_connection_id.is_some() {
            "编辑连接"
        } else {
            "新建连接"
        };
        let palette = mac_dialog_palette(ctx.style().visuals.dark_mode);
        egui::Window::new(if self.editing_connection_id.is_some() {
            "编辑连接"
        } else {
            "新建连接"
        })
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .title_bar(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_width(580.0)
        .min_width(580.0)
        .frame(
            egui::Frame::new()
                .fill(palette.window_bg)
                .stroke(Stroke::new(1.0, palette.border))
                .corner_radius(16.0)
                .inner_margin(egui::Margin::symmetric(22, 20)),
        )
        .open(&mut open)
        .show(ctx, |ui| {
            ui.scope(|ui| {
                apply_mac_dialog_style(ui, palette);
                ui.set_width(580.0);
                ui.spacing_mut().item_spacing = egui::vec2(14.0, 12.0);
                ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);

                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(22.0).strong().color(palette.title));
                    ui.add_space(14.0);
                    ui.small(RichText::new("配置数据库连接信息").color(palette.subtitle));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("关闭").size(12.0).color(palette.subtitle))
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            should_close = true;
                        }
                    });
                });
                ui.add_space(10.0);

                egui::Frame::new()
                    .fill(palette.section_bg)
                    .stroke(Stroke::new(1.0, palette.section_border))
                    .corner_radius(12.0)
                    .inner_margin(egui::Margin::symmetric(18, 16))
                    .show(ui, |ui| {
                        egui::Grid::new("connection-form-grid")
                            .num_columns(2)
                            .spacing([16.0, 12.0])
                            .min_col_width(108.0)
                            .show(ui, |ui| {
                                form_grid_row(ui, "数据库", |ui| {
                                    egui::ComboBox::from_id_salt("db-kind")
                                        .selected_text(match self.connection_form.kind {
                                            DatabaseKind::MySql => "MySQL",
                                            DatabaseKind::Postgres => "PostgreSQL",
                                        })
                                        .width(380.0)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    matches!(self.connection_form.kind, DatabaseKind::MySql),
                                                    "MySQL",
                                                )
                                                .clicked()
                                            {
                                                self.connection_form.kind = DatabaseKind::MySql;
                                                self.connection_form.port = 3306;
                                            }
                                            if ui
                                                .selectable_label(
                                                    matches!(self.connection_form.kind, DatabaseKind::Postgres),
                                                    "PostgreSQL",
                                                )
                                                .clicked()
                                            {
                                                self.connection_form.kind = DatabaseKind::Postgres;
                                                self.connection_form.port = 5432;
                                            }
                                        });
                                });
                                form_row(ui, "名称", &mut self.connection_form.name);
                                form_row(ui, "分组", &mut self.connection_form.group_name);
                                form_row(ui, "主机", &mut self.connection_form.host);
                                form_row_u16(ui, "端口", &mut self.connection_form.port);
                                form_row(ui, "用户名", &mut self.connection_form.username);
                                form_grid_row(ui, "密码", |ui| {
                                    ui.add_sized(
                                        [380.0, 30.0],
                                        TextEdit::singleline(&mut self.connection_form.password).password(true),
                                    );
                                });
                                form_row(ui, "默认数据库", &mut self.connection_form.default_database);
                                form_grid_row(ui, "超时(秒)", |ui| {
                                    ui.add_sized(
                                        [120.0, 30.0],
                                        egui::DragValue::new(&mut self.connection_form.connect_timeout_secs)
                                            .range(1..=60),
                                    );
                                });
                                form_grid_row(ui, "SSL", |ui| {
                                    egui::ComboBox::from_id_salt("ssl-mode")
                                        .selected_text(match self.connection_form.ssl_mode {
                                            SslMode::Disable => "Disable",
                                            SslMode::Prefer => "Prefer",
                                            SslMode::Require => "Require",
                                        })
                                        .width(140.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut self.connection_form.ssl_mode,
                                                SslMode::Disable,
                                                "Disable",
                                            );
                                            ui.selectable_value(
                                                &mut self.connection_form.ssl_mode,
                                                SslMode::Prefer,
                                                "Prefer",
                                            );
                                            ui.selectable_value(
                                                &mut self.connection_form.ssl_mode,
                                                SslMode::Require,
                                                "Require",
                                            );
                                        });
                                });
                            });
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.connection_form.save_password, "保存密码");
                    ui.add_space(16.0);
                    ui.checkbox(&mut self.connection_form.ssh_enabled, "启用 SSH Tunnel");
                });

                if self.connection_form.ssh_enabled {
                    ui.add_space(8.0);
                    egui::Frame::new()
                        .fill(palette.section_bg)
                        .stroke(Stroke::new(1.0, palette.section_border))
                        .corner_radius(12.0)
                        .inner_margin(egui::Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            ui.small(RichText::new("SSH Tunnel").color(palette.subtitle));
                            ui.add_space(6.0);
                            egui::Grid::new("ssh-form-grid")
                                .num_columns(2)
                                .spacing([16.0, 12.0])
                                .min_col_width(108.0)
                                .show(ui, |ui| {
                                    form_row(ui, "SSH 主机", &mut self.connection_form.ssh_host);
                                    form_row_u16(ui, "SSH 端口", &mut self.connection_form.ssh_port);
                                    form_row(ui, "SSH 用户", &mut self.connection_form.ssh_username);
                                });
                        });
                }

                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if dialog_button(ui, "保存连接", true).clicked() {
                        self.save_connection_form();
                    }
                    ui.add_space(8.0);
                    if dialog_button(ui, "测试连接", false).clicked() {
                        self.test_connection_form();
                    }
                });
            });
        });
        if should_close || !self.is_connection_dialog_open {
            self.is_connection_dialog_open = false;
        }
    }

    fn render_delete_confirm_dialog(&mut self, ctx: &egui::Context) {
        let saved_query_pending = self.pending_saved_query_delete.clone();
        let table_rows_pending = self.pending_delete_confirmation.clone();
        let is_saved_query_delete = saved_query_pending.is_some();
        let is_table_rows_delete = table_rows_pending.is_some();
        if !is_saved_query_delete && !is_table_rows_delete {
            return;
        }
        let palette = mac_dialog_palette(ctx.style().visuals.dark_mode);
        let mut should_close = false;
        let mut should_confirm = false;

        let message = if is_saved_query_delete {
            format!("确认要删除「{}」吗？", saved_query_pending.as_ref().unwrap().title)
        } else {
            "确认要删除吗？".to_string()
        };

        // Dismiss on Escape
        ctx.input_mut(|input| {
            if input.key_pressed(egui::Key::Escape) {
                should_close = true;
            }
        });

        egui::Area::new("delete-confirm".into())
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .interactable(true)
            .show(ctx, |ui| {
                let max_rect = ui.max_rect();
                let w = if is_saved_query_delete { 320.0 } else { 258.0 };
                let h = 38.0;
                let rect = egui::Rect::from_center_size(max_rect.center(), egui::vec2(w, h));
                ui.allocate_ui_at_rect(rect, |ui| {
                    apply_mac_dialog_style(ui, palette);
                    ui.painter().rect_filled(ui.max_rect(), 7.0, palette.window_bg);
                    ui.painter().rect_stroke(ui.max_rect(), 7.0, Stroke::new(1.0, palette.border), egui::StrokeKind::Outside);
                    let inner = ui.max_rect().shrink2(egui::vec2(10.0, 4.0));
                    ui.allocate_ui_at_rect(inner, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
                        ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                        ui.horizontal_centered(|ui| {
                            ui.label(RichText::new(&message).size(12.5).color(palette.title));
                            if dialog_button(ui, "取消", false).clicked() {
                                should_close = true;
                            }
                            if dialog_button(ui, "确认删除", true).clicked() {
                                should_confirm = true;
                                should_close = true;
                            }
                        });
                    });
                });
            });

        if should_confirm {
            if is_saved_query_delete {
                self.confirm_delete_saved_query();
            } else {
                self.confirm_pending_delete_rows();
                self.pending_delete_confirmation = None;
            }
        } else if should_close {
            self.pending_saved_query_delete = None;
            self.pending_delete_confirmation = None;
        }
    }

    fn render_saved_query_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.pending_saved_query_dialog.as_mut() else {
            return;
        };
        let palette = mac_dialog_palette(ctx.style().visuals.dark_mode);
        let mut should_close = false;
        let mut confirmed = false;

        // Dismiss on Escape
        ctx.input_mut(|input| {
            if input.key_pressed(egui::Key::Escape) {
                should_close = true;
            }
        });

        let dialog_title = match dialog.mode {
            SavedQueryDialogMode::Save => "保存查询",
            SavedQueryDialogMode::Update { .. } => "更新查询",
            SavedQueryDialogMode::Rename { .. } => "重命名查询",
        };
        let button_label = match dialog.mode {
            SavedQueryDialogMode::Save => "保存",
            SavedQueryDialogMode::Update { .. } => "更新",
            SavedQueryDialogMode::Rename { .. } => "重命名",
        };
        let connection_id = dialog.connection_id.clone();
        let mode = dialog.mode.clone();

        egui::Window::new(dialog_title)
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .title_bar(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(400.0)
            .min_width(400.0)
            .frame(
                egui::Frame::new()
                    .fill(palette.window_bg)
                    .stroke(Stroke::new(1.0, palette.border))
                    .corner_radius(16.0)
                    .inner_margin(egui::Margin::symmetric(22, 20)),
            )
            .open(&mut true)
            .show(ctx, |ui| {
                ui.scope(|ui| {
                    apply_mac_dialog_style(ui, palette);
                    ui.set_width(400.0);
                    ui.spacing_mut().item_spacing = egui::vec2(14.0, 12.0);
                    ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new(dialog_title).size(18.0).strong().color(palette.title));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("✕").size(14.0).color(palette.subtitle))
                                        .fill(Color32::TRANSPARENT)
                                        .stroke(Stroke::NONE),
                                )
                                .clicked()
                            {
                                should_close = true;
                            }
                        });
                    });
                    ui.add_space(12.0);

                    ui.label(RichText::new("查询名称").size(13.0).color(palette.weak_text));
                    let input_response = ui.add(
                        egui::TextEdit::singleline(&mut dialog.title_input)
                            .hint_text("输入查询名称")
                            .font(FontId::new(14.0, FontFamily::Proportional))
                            .desired_width(ui.available_width()),
                    );
                    input_response.request_focus();

                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if dialog_button(ui, button_label, true).clicked() {
                                confirmed = true;
                                should_close = true;
                            }
                            if dialog_button(ui, "取消", false).clicked() {
                                should_close = true;
                            }
                        });
                    });

                    if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                        confirmed = true;
                        should_close = true;
                    }
                });
            });

        // Clone title before dropping the mutable borrow
        let confirmed_title = dialog.title_input.trim().to_string();
        if should_close {
            self.pending_saved_query_dialog = None;
        }

        if confirmed {
            if confirmed_title.is_empty() {
                self.status_message = "查询名称不能为空".into();
                if let Some(WorkspaceTab::Query(tab)) = self.tabs.get_mut(self.active_tab) {
                    tab.messages.push("保存失败：查询名称不能为空".into());
                    tab.active_bottom_tab = QueryBottomTab::Messages;
                }
            } else {
                match mode {
                    SavedQueryDialogMode::Save => {
                        self.save_active_query(&connection_id, &confirmed_title);
                    }
                    SavedQueryDialogMode::Update { .. } => {
                        self.save_active_query(&connection_id, &confirmed_title);
                    }
                    SavedQueryDialogMode::Rename { entry_id } => {
                        self.rename_saved_query(&entry_id, &connection_id, &confirmed_title);
                    }
                }
            }
        }
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background_tasks();

        // Two-phase refresh: first frame shows "正在刷新", next frame does the work
        if let Some(reload_definition) = self.pending_refresh_active_table.take() {
            self.refresh_active_table_preview(reload_definition);
            ctx.request_repaint();
        }

        if self.pending_connection_tree.is_some()
            || self.pending_query_execution.is_some()
            || self.pending_database_list.is_some()
        {
            // 后台任务进行中时主动请求后续帧，避免必须等鼠标再次移动才显示结果。
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        ctx.set_visuals(app_visuals(self.use_dark_theme));
        let style = app_style(ctx.style().as_ref());
        ctx.set_style(style);

        egui::TopBottomPanel::top("toolbar")
            .exact_height(40.0)
            .frame(
                egui::Frame::NONE
                    .fill(mac_ui_palette(&ctx.style().visuals).toolbar_bg)
                    .inner_margin(egui::vec2(0.0, 7.0)),
            )
            .show(ctx, |ui| self.render_toolbar(ui));
        let palette = if ctx.style().visuals.dark_mode {
            mac_sidebar_palette_dark()
        } else {
            mac_sidebar_palette_light()
        };
        let sidebar = egui::SidePanel::left("sidebar")
            .resizable(false)
            .default_width(self.sidebar_width)
            .min_width(180.0)
            .max_width(300.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(palette.sidebar_bg))
            .show(ctx, |ui| self.render_sidebar(ui));
        self.sidebar_width = sidebar.response.rect.width().clamp(180.0, 300.0);
        if ctx.input(|input| input.pointer.any_pressed()) && !sidebar.response.hovered() {
            self.sidebar_has_focus = false;
        }
        // --- keyboard shortcut: Cmd+C ---
        // Use Event::Copy so it works even when TextEdit consumes the key event first
        let mut cmd_c = ctx.input(|input| input.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        // Also check raw shortcut (for sidebar case, or when TextEdit doesn't have focus)
        if !cmd_c {
            cmd_c = ctx.input_mut(|input| {
                input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::C,
                )) || (input.modifiers.command && input.key_pressed(egui::Key::C))
            });
        }
        if cmd_c {
            if self.sidebar_has_focus && self.selected_tree_item.is_some() {
                let _ = self.copy_selected_sidebar_item(ctx);
            } else if !self.sidebar_has_focus {
                self.copy_selected_columns(ctx);
            }
        }

        // ESC: clear column selection and row selection when sidebar doesn't have focus
        let esc_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                || input.key_pressed(egui::Key::Escape)
        });
        if esc_pressed && !self.sidebar_has_focus {
            self.clear_column_and_row_selection();
        }

        let open_sidebar_selection = self.sidebar_has_focus
            && self.selected_tree_item.is_some()
            && ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || input.key_pressed(egui::Key::Enter)
            });
        if open_sidebar_selection {
            let _ = self.open_selected_sidebar_item();
        }
        let refresh_active_workspace = ctx.input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::R,
            )) || (input.modifiers.command && input.key_pressed(egui::Key::R))
        });
        if refresh_active_workspace {
            self.refresh_active_workspace();
        }
        // Cmd+S: save active query
        let save_query = ctx.input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            )) || (input.modifiers.command && input.key_pressed(egui::Key::S))
        });
        if save_query {
            let connection_id = self
                .tabs
                .get(self.active_tab)
                .and_then(|tab| match tab {
                    WorkspaceTab::Query(t) => t
                        .connection_id
                        .clone()
                        .or_else(|| self.selected_connection.clone()),
                    _ => None,
                });
            if let Some(cid) = connection_id {
                self.open_save_query_dialog(&cid);
            } else {
                self.status_message = "请先选择一个连接后再保存查询".into();
            }
        }
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(24.0)
            .show(ctx, |ui| self.render_status_bar(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.render_tabs(ui));
        self.render_connection_dialog(ctx);
        self.render_delete_confirm_dialog(ctx);
        self.render_saved_query_dialog(ctx);

        // Keyboard shortcut: close current tab with Cmd/Ctrl+W
        let close_tab = ctx.input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
                egui::Key::W,
            )) || (input.modifiers.command && input.modifiers.shift && input.key_pressed(egui::Key::W))
        });
        if close_tab {
            self.close_workspace_tab(self.active_tab);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.services.save_ui_state(
            "selected_connection",
            self.selected_connection.as_deref().unwrap_or(""),
        );
        let _ = self
            .services
            .save_ui_state("sidebar_width", &format!("{:.1}", self.sidebar_width));
        let _ = self
            .services
            .save_ui_state("theme", if self.use_dark_theme { "dark" } else { "light" });
    }
}

impl QueryTabState {
    fn new(connection_id: Option<String>) -> Self {
        Self {
            id: format!("query-{}", uuid::Uuid::new_v4()),
            title: "SQL 查询".into(),
            connection_id,
            database: None,
            sql: String::new(),
            cursor_range: None,
            result: None,
            history: Vec::new(),
            saved_queries: Vec::new(),
            messages: vec!["查询工作台已就绪".into()],
            error: None,
            active_bottom_tab: QueryBottomTab::Results,
            last_executed_sql: None,
            result_sort: TableSortState::default(),
            selected_columns: BTreeSet::new(),
            multi_results: Vec::new(),
            selected_result_index: 0,
            editor_focus_requested: true,
            editor_height: None,
            saved_queries_panel_visible: true,
            saved_queries_filter: String::new(),
            selected_saved_query_id: None,
        }
    }
}

// #region debug-point shared:reporter
fn debug_report(run_id: &str, hypothesis_id: &str, location: &str, msg: &str, data: String) {
    let env_path = ".dbg/startup-spinner.env";
    let env_content = std::fs::read_to_string(env_path).unwrap_or_default();
    let server_url = env_content
        .lines()
        .find_map(|line| line.strip_prefix("DEBUG_SERVER_URL="))
        .unwrap_or("http://127.0.0.1:7777/event");
    let session_id = env_content
        .lines()
        .find_map(|line| line.strip_prefix("DEBUG_SESSION_ID="))
        .unwrap_or("startup-spinner");
    let body = format!(
        "{{\"sessionId\":\"{}\",\"runId\":\"{}\",\"hypothesisId\":\"{}\",\"location\":\"{}\",\"msg\":\"{}\",\"data\":\"{}\",\"ts\":{}}}",
        debug_escape_json(session_id),
        debug_escape_json(run_id),
        debug_escape_json(hypothesis_id),
        debug_escape_json(location),
        debug_escape_json(msg),
        debug_escape_json(&data),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default()
    );
    if let Some((host, port, path)) = debug_parse_http_url(server_url) {
        use std::io::Write as _;
        use std::net::TcpStream;
        if let Ok(mut stream) = TcpStream::connect((host.as_str(), port)) {
            let request = format!(
                "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                path,
                host,
                body.len(),
                body
            );
            let _ = stream.write_all(request.as_bytes());
        }
    }
}

fn debug_parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let raw = url.strip_prefix("http://")?;
    let (host_port, path) = match raw.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{}", path)),
        None => (raw, "/event".to_string()),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().ok()?),
        None => (host_port.to_string(), 80),
    };
    Some((host, port, path))
}

fn debug_escape_json(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}
// #endregion

impl ConnectionFormState {
    fn from_profile(profile: &ConnectionProfile) -> Self {
        Self {
            name: profile.name.clone(),
            kind: profile.kind,
            group_name: profile.group_name.clone().unwrap_or_default(),
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            password: String::new(),
            default_database: profile.default_database.clone().unwrap_or_default(),
            save_password: profile.password_saved,
            connect_timeout_secs: profile.connect_timeout_secs,
            ssl_mode: profile.ssl_mode,
            ssh_enabled: profile.ssh_tunnel.as_ref().map(|item| item.enabled).unwrap_or(false),
            ssh_host: profile
                .ssh_tunnel
                .as_ref()
                .map(|item| item.host.clone())
                .unwrap_or_default(),
            ssh_port: profile
                .ssh_tunnel
                .as_ref()
                .map(|item| item.port)
                .unwrap_or(22),
            ssh_username: profile
                .ssh_tunnel
                .as_ref()
                .map(|item| item.username.clone())
                .unwrap_or_default(),
        }
    }

    fn to_input(&self) -> ConnectionProfileInput {
        let has_password = !self.password.trim().is_empty();
        ConnectionProfileInput {
            name: self.name.clone(),
            kind: self.kind,
            group_name: optional_string(&self.group_name),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            password: optional_string(&self.password),
            default_database: optional_string(&self.default_database),
            save_password: self.save_password || has_password,
            connect_timeout_secs: self.connect_timeout_secs,
            ssl_mode: self.ssl_mode,
            ssh_tunnel: if self.ssh_enabled {
                Some(core_domain::SshTunnelConfig {
                    enabled: true,
                    host: self.ssh_host.clone(),
                    port: self.ssh_port,
                    username: self.ssh_username.clone(),
                })
            } else {
                None
            },
        }
    }
}

enum SidebarAction {
    OpenConnection(String),
    ToggleNode(String, ExplorerNode),
    OpenTable(ExplorerNode),
    RefreshConnection(String),
    RefreshNode(String, ExplorerNode),
}

enum TabUiAction {
    None,
    ExecuteQuery(ExecuteMode),
    RefreshQueryHistory(String),
    OpenSaveQueryDialog(String),
    OpenRenameSavedQueryDialog(SavedQueryEntry),
    PromptDeleteSavedQuery(SavedQueryEntry),
    RefreshActiveTable { reload_definition: bool },
    ExportActiveResult,
    CopyTextToClipboard {
        text: String,
        status_message: String,
    },
    SaveActiveTableCellEdit {
        row_index: usize,
        column: String,
        value: String,
        is_null: bool,
    },
    SavePendingInsertRow,
    DeleteActiveTableRows(Vec<usize>),
    CopyActiveTableRowsAsInsert(Vec<usize>),
    CopyActiveTableRowsAsTsv(Vec<usize>),
    NewQueryFromTable {
        connection_id: String,
        database: Option<String>,
        schema: Option<String>,
        table: String,
    },
}

#[derive(Clone)]
enum ExecuteMode {
    Whole,
    Selection(Option<String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QueryBottomTab {
    Results,
    Messages,
    History,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableViewMode {
    Data,
    Structure,
    Definition,
}

#[derive(Clone, Copy)]
enum ToolbarButtonKind {
    Primary,
    Secondary,
    Accent,
    Subtle,
}

#[derive(Clone, Copy)]
enum MiniButtonKind {
    Subtle,
    Danger,
    Accent,
}

#[derive(Clone, Copy)]
enum StatusLevel {
    Normal,
    Pending,
    Success,
    Error,
}

#[derive(Clone, Copy)]
struct MacUiPalette {
    toolbar_bg: Color32,
    sidebar_bg: Color32,
    workspace_bg: Color32,
    card_bg: Color32,
    table_header_bg: Color32,
    table_alt_bg: Color32,
    search_bg: Color32,
    border: Color32,
    soft_border: Color32,
    table_grid: Color32,
    selection_bg: Color32,
    selection_stroke: Color32,
    selection_text: Color32,
    text: Color32,
    weak_text: Color32,
    muted_dot: Color32,
    success: Color32,
    danger: Color32,
    warning: Color32,
    tab_idle_bg: Color32,
    primary_button_bg: Color32,
    primary_button_stroke: Color32,
    primary_button_text: Color32,
    secondary_button_bg: Color32,
    secondary_button_stroke: Color32,
    secondary_button_text: Color32,
    accent_button_bg: Color32,
    accent_button_stroke: Color32,
    accent_button_text: Color32,
    subtle_button_bg: Color32,
    subtle_button_stroke: Color32,
    subtle_button_text: Color32,
    danger_button_bg: Color32,
    danger_button_stroke: Color32,
    danger_button_text: Color32,
}

impl QueryBottomTab {
    fn label(self) -> &'static str {
        match self {
            Self::Results => "结果",
            Self::Messages => "消息",
            Self::History => "历史",
        }
    }
}

impl TableViewMode {
    fn label(self) -> &'static str {
        match self {
            Self::Data => "数据",
            Self::Structure => "结构",
            Self::Definition => "DDL",
        }
    }
}

fn render_result_table(
    ui: &mut egui::Ui,
    result: &mut QueryResult,
    sort_state: &mut TableSortState,
    sql_driven_sort: bool,
    selected_columns: &mut BTreeSet<String>,
) -> Option<(String, bool)> {
    let palette = mac_ui_palette(ui.visuals());
    if result.columns.is_empty() {
        ui.label("当前语句没有结果集");
        return None;
    }

    let viewport_width = ui.available_width().max(0.0);
    let viewport_height = ui.available_height().max(220.0);
    let mut sort_click_result = None;

    egui::Frame::new()
        .fill(palette.card_bg)
        .stroke(Stroke::new(1.0, palette.soft_border))
        .show(ui, |ui| {
            ui.set_width(viewport_width);
            ui.set_min_height(viewport_height);
            egui::ScrollArea::both()
                .id_salt(format!(
                    "result-grid-{}-{}",
                    result.columns.len(),
                    result.rows.len()
                ))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    let mut selected_sort = None;
                    let modifiers = ui.ctx().input(|input| input.modifiers);
                    let ctrl_held = modifiers.ctrl || modifiers.command;
                    let column_widths = estimate_result_column_widths(result);
                    let mut table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
                    for width in &column_widths {
                        table = table.column(
                            egui_extras::Column::initial(*width)
                                .at_least(72.0)
                                .clip(true),
                        );
                    }
                    table.header(30.0, |mut header| {
                            for column in &result.columns {
                                header.col(|ui| {
                                    let is_selected = selected_columns.contains(column);
                                    let (sort_choice, clicked) = table_header_cell(
                                        ui,
                                        &palette,
                                        column,
                                        true,
                                        sort_indicator(sort_state, column),
                                        is_selected,
                                    );
                                    if let Some(choice) = sort_choice {
                                        selected_sort = Some((column.clone(), choice));
                                    }
                                    if clicked {
                                        if ctrl_held {
                                            if selected_columns.contains(column) {
                                                selected_columns.remove(column);
                                            } else {
                                                selected_columns.insert(column.clone());
                                            }
                                        } else {
                                            selected_columns.clear();
                                            selected_columns.insert(column.clone());
                                        }
                                        ui.ctx().request_repaint();
                                    }
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(28.0, result.rows.len(), |mut row_ui| {
                                let index = row_ui.index();
                                let fill = if index % 2 == 0 {
                                    palette.card_bg
                                } else {
                                    palette.table_alt_bg
                                };
                                let row = &result.rows[index];
                                for column in &result.columns {
                                    row_ui.col(|ui| {
                                        let column_selected = selected_columns.contains(column);
                                        table_body_cell(
                                            ui,
                                            &palette,
                                            fill,
                                            row.get(column).unwrap_or(&QueryCellValue::Null),
                                            false,
                                            column_selected,
                                        );
                                    });
                                }
                            });
                        });
                    if let Some(sort_request) = selected_sort {
                        sort_click_result = Some(sort_request);
                    }
                });
        });
    if let Some((column, choice)) = sort_click_result {
        match choice {
            TableHeaderSortChoice::Clear => {
                clear_table_sort_state(sort_state);
                return None;
            }
            TableHeaderSortChoice::Ascending | TableHeaderSortChoice::Descending => {
                if sql_driven_sort {
                    return Some((
                        column,
                        matches!(choice, TableHeaderSortChoice::Descending),
                    ));
                }
                apply_table_sort_choice(
                    result,
                    sort_state,
                    &column,
                    matches!(choice, TableHeaderSortChoice::Descending),
                );
            }
        }
    }
    None
}

fn render_editable_table(ui: &mut egui::Ui, tab: &mut TableTabState) -> TabUiAction {
    let palette = mac_ui_palette(ui.visuals());
    let Some(preview) = tab.preview.as_ref() else {
        ui.label("暂无预览数据");
        return TabUiAction::None;
    };

    let viewport_width = ui.available_width().max(0.0);
    let viewport_height = ui.available_height().max(220.0);
    let columns = table_editable_columns(tab);
    if columns.is_empty() {
        return TabUiAction::None;
    }
    let row_count = preview.rows.len();
    let column_widths = if tab.preview_column_widths.len() == columns.len() {
        tab.preview_column_widths.clone()
    } else {
        let widths = estimate_query_column_widths(&columns, &preview.rows);
        tab.preview_column_widths = widths.clone();
        widths
    };
    let editable_columns = table_editable_columns(tab);
    let mut action = TabUiAction::None;
    let mut selected_sort = None;
    let mut should_cancel_pending_insert = false;

    egui::Frame::new()
        .fill(palette.card_bg)
        .stroke(Stroke::new(1.0, palette.soft_border))
        .show(ui, |ui| {
            ui.set_width(viewport_width);
            ui.set_min_height(viewport_height);
            egui::ScrollArea::both()
                .id_salt(format!("editable-table-grid-{}", tab.title))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    let modifiers = ui.ctx().input(|input| input.modifiers);
                    let ctrl_held = modifiers.ctrl || modifiers.command;
                    let mut table = TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
                    for width in &column_widths {
                        table = table.column(
                            egui_extras::Column::initial(*width)
                                .at_least(72.0)
                                .clip(true),
                        );
                    }
                    table
                        .header(30.0, |mut header| {
                            for column in &columns {
                                header.col(|ui| {
                                    let is_selected = tab.selected_columns.contains(column);
                                    let (sort_choice, clicked) = table_header_cell(
                                        ui,
                                        &palette,
                                        column,
                                        true,
                                        sort_indicator(&tab.preview_sort, column),
                                        is_selected,
                                    );
                                    if let Some(choice) = sort_choice {
                                        selected_sort = Some((column.clone(), choice));
                                    }
                                    if clicked {
                                        if ctrl_held {
                                            if tab.selected_columns.contains(column) {
                                                tab.selected_columns.remove(column);
                                            } else {
                                                tab.selected_columns.insert(column.clone());
                                            }
                                        } else {
                                            tab.selected_columns.clear();
                                            tab.selected_columns.insert(column.clone());
                                        }
                                        ui.ctx().request_repaint();
                                    }
                                });
                            }
                        })
                        .body(|mut body| {
                            if tab.pending_insert_row.is_none() {
                                body.rows(28.0, row_count, |mut row_ui| {
                                    let row_index = row_ui.index();
                                    let row_selected = table_row_is_selected(tab, row_index);
                                    let fill = table_row_fill(
                                        &palette,
                                        row_index,
                                        row_selected,
                                        false,
                                    );
                                    for column in &columns {
                                        row_ui.col(|ui| {
                                            let cell_value = tab
                                                .preview
                                                .as_ref()
                                                .and_then(|preview| preview.rows.get(row_index))
                                                .and_then(|row| row.get(column))
                                                .cloned()
                                                .unwrap_or_default();
                                            let is_editing = matches!(
                                                tab.editing_cell.as_ref(),
                                                Some(edit)
                                                    if edit.target == TableEditTarget::ExistingRow(row_index)
                                                        && edit.column == *column
                                            );
                                            let column_selected = tab.selected_columns.contains(column);
                                            let response = if is_editing {
                                                let edit = tab.editing_cell.as_mut().expect("edit state");
                                                render_table_editor_cell(
                                                    ui,
                                                    &palette,
                                                    fill,
                                                    edit,
                                                    false,
                                                    row_selected,
                                                )
                                            } else {
                                                render_table_body_interactive_cell(
                                                    ui,
                                                    &palette,
                                                    fill,
                                                    &cell_value,
                                                    false,
                                                    row_selected,
                                                    column_selected,
                                                )
                                            };
                                            let modifiers = ui.ctx().input(|input| input.modifiers);
                                            let toggle_select = modifiers.ctrl || modifiers.command;
                                            let range_select = modifiers.shift;
                                            if response.secondary_clicked() {
                                                if !table_row_is_selected(tab, row_index) {
                                                    set_single_preview_selection(tab, row_index);
                                                } else {
                                                    tab.selected_preview_row = Some(row_index);
                                                    normalize_preview_selection(tab);
                                                }
                                            }
                                            if !is_editing && response.clicked() {
                                                if range_select {
                                                    extend_preview_selection(tab, row_index);
                                                    tab.editing_cell = None;
                                                } else if toggle_select {
                                                    toggle_preview_selection(tab, row_index);
                                                    tab.editing_cell = None;
                                                } else {
                                                    set_single_preview_selection(tab, row_index);
                                                    tab.editing_cell = Some(TableCellEditState {
                                                        target: TableEditTarget::ExistingRow(row_index),
                                                        column: column.clone(),
                                                        value: cell_value
                                                            .as_text()
                                                            .unwrap_or_default()
                                                            .to_string(),
                                                        is_null: cell_value.is_null(),
                                                        focus_requested: true,
                                                    });
                                                }
                                                ui.ctx().request_repaint();
                                            }
                                            if is_editing {
                                                let enter_pressed = ui.ctx().input(|input| {
                                                    input.key_pressed(egui::Key::Enter)
                                                });
                                                if enter_pressed {
                                                    let (edited_value, edited_is_null) = tab
                                                        .editing_cell
                                                        .as_ref()
                                                        .map(|edit| (edit.value.clone(), edit.is_null))
                                                        .unwrap_or_default();
                                                    tab.editing_cell = None;
                                                    if edited_is_null != cell_value.is_null()
                                                        || edited_value
                                                            != cell_value
                                                                .as_text()
                                                                .unwrap_or_default()
                                                    {
                                                        action = TabUiAction::SaveActiveTableCellEdit {
                                                            row_index,
                                                            column: column.clone(),
                                                            value: edited_value,
                                                            is_null: edited_is_null,
                                                        };
                                                    }
                                                } else if response.lost_focus() {
                                                    tab.editing_cell = None;
                                                }
                                            }
                                            response.context_menu(|ui| {
                                                if !table_row_is_selected(tab, row_index) {
                                                    set_single_preview_selection(tab, row_index);
                                                } else {
                                                    tab.selected_preview_row = Some(row_index);
                                                    normalize_preview_selection(tab);
                                                }
                                                let selected_row_indices =
                                                    preview_selected_row_indices(tab, row_index);
                                                let selected_count = selected_row_indices.len();
                                                if ui.button("添加记录").clicked() {
                                                    tab.pending_insert_row =
                                                        Some(create_empty_insert_row(&editable_columns));
                                                    tab.editing_cell = editable_columns.first().map(|first| {
                                                        TableCellEditState {
                                                            target: TableEditTarget::PendingInsert,
                                                            column: first.clone(),
                                                            value: String::new(),
                                                            is_null: false,
                                                            focus_requested: true,
                                                        }
                                                    });
                                                    ui.close();
                                                }
                                                if ui.button("设置为空白字符串").clicked() {
                                                    action = TabUiAction::SaveActiveTableCellEdit {
                                                        row_index,
                                                        column: column.clone(),
                                                        value: String::new(),
                                                        is_null: false,
                                                    };
                                                    ui.close();
                                                }
                                                if ui.button("设置为 NULL").clicked() {
                                                    action = TabUiAction::SaveActiveTableCellEdit {
                                                        row_index,
                                                        column: column.clone(),
                                                        value: String::new(),
                                                        is_null: true,
                                                    };
                                                    ui.close();
                                                }
                                                if ui
                                                    .add_enabled(
                                                        !tab.table.is_view,
                                                        egui::Button::new(if selected_count > 1 {
                                                            format!("删除选中 {} 条记录", selected_count)
                                                        } else {
                                                            "删除记录".into()
                                                        }),
                                                    )
                                                    .clicked()
                                                {
                                                    action = TabUiAction::DeleteActiveTableRows(
                                                        selected_row_indices.clone(),
                                                    );
                                                    ui.close();
                                                }
                                                let copy_tsv_label = if selected_count > 1 {
                                                    format!("复制选中 {} 条数据", selected_count)
                                                } else {
                                                    "复制数据".into()
                                                };
                                                if ui.button(copy_tsv_label).clicked() {
                                                    action =
                                                        TabUiAction::CopyActiveTableRowsAsTsv(
                                                            selected_row_indices.clone(),
                                                        );
                                                    ui.close();
                                                }
                                                let copy_label = if selected_count > 1 {
                                                    format!("复制选中 {} 条为 INSERT", selected_count)
                                                } else {
                                                    "复制为 INSERT 语句".into()
                                                };
                                                if ui.button(copy_label).clicked() {
                                                    action =
                                                        TabUiAction::CopyActiveTableRowsAsInsert(
                                                            selected_row_indices,
                                                        );
                                                    ui.close();
                                                }
                                            });
                                        });
                                    }
                                });
                            } else {
                                for row_index in 0..row_count {
                                    let row_selected = table_row_is_selected(tab, row_index);
                                    let fill = table_row_fill(
                                        &palette,
                                        row_index,
                                        row_selected,
                                        false,
                                    );
                                    body.row(28.0, |mut row_ui| {
                                        for column in &columns {
                                            row_ui.col(|ui| {
                                                let cell_value = tab
                                                    .preview
                                                    .as_ref()
                                                    .and_then(|preview| preview.rows.get(row_index))
                                                    .and_then(|row| row.get(column))
                                                    .cloned()
                                                    .unwrap_or_default();
                                                let is_editing = matches!(
                                                    tab.editing_cell.as_ref(),
                                                    Some(edit)
                                                        if edit.target == TableEditTarget::ExistingRow(row_index)
                                                            && edit.column == *column
                                                );
                                                let column_selected = tab.selected_columns.contains(column);
                                                let response = if is_editing {
                                                    let edit = tab.editing_cell.as_mut().expect("edit state");
                                                    render_table_editor_cell(
                                                        ui,
                                                        &palette,
                                                        fill,
                                                        edit,
                                                        false,
                                                        row_selected,
                                                    )
                                                } else {
                                                    render_table_body_interactive_cell(
                                                        ui,
                                                        &palette,
                                                        fill,
                                                        &cell_value,
                                                        false,
                                                        row_selected,
                                                        column_selected,
                                                    )
                                                };
                                                let modifiers = ui.ctx().input(|input| input.modifiers);
                                                let toggle_select = modifiers.ctrl || modifiers.command;
                                                let range_select = modifiers.shift;
                                                if response.secondary_clicked() {
                                                    if !table_row_is_selected(tab, row_index) {
                                                        set_single_preview_selection(tab, row_index);
                                                    } else {
                                                        tab.selected_preview_row = Some(row_index);
                                                        normalize_preview_selection(tab);
                                                    }
                                                }
                                                if !is_editing && response.clicked() {
                                                    if range_select {
                                                        extend_preview_selection(tab, row_index);
                                                        tab.editing_cell = None;
                                                    } else if toggle_select {
                                                        toggle_preview_selection(tab, row_index);
                                                        tab.editing_cell = None;
                                                    } else {
                                                        set_single_preview_selection(tab, row_index);
                                                        tab.editing_cell = Some(TableCellEditState {
                                                            target: TableEditTarget::ExistingRow(row_index),
                                                            column: column.clone(),
                                                            value: cell_value
                                                                .as_text()
                                                                .unwrap_or_default()
                                                                .to_string(),
                                                            is_null: cell_value.is_null(),
                                                            focus_requested: true,
                                                        });
                                                    }
                                                    ui.ctx().request_repaint();
                                                }
                                                if is_editing {
                                                    let enter_pressed = ui.ctx().input(|input| {
                                                        input.key_pressed(egui::Key::Enter)
                                                    });
                                                    if enter_pressed {
                                                        let (edited_value, edited_is_null) = tab
                                                            .editing_cell
                                                            .as_ref()
                                                            .map(|edit| (edit.value.clone(), edit.is_null))
                                                            .unwrap_or_default();
                                                        tab.editing_cell = None;
                                                        if edited_is_null != cell_value.is_null()
                                                            || edited_value
                                                                != cell_value
                                                                    .as_text()
                                                                    .unwrap_or_default()
                                                        {
                                                            action = TabUiAction::SaveActiveTableCellEdit {
                                                                row_index,
                                                                column: column.clone(),
                                                                value: edited_value,
                                                                is_null: edited_is_null,
                                                            };
                                                        }
                                                    } else if response.lost_focus() {
                                                        tab.editing_cell = None;
                                                    }
                                                }
                                                response.context_menu(|ui| {
                                                    if !table_row_is_selected(tab, row_index) {
                                                        set_single_preview_selection(tab, row_index);
                                                    } else {
                                                        tab.selected_preview_row = Some(row_index);
                                                        normalize_preview_selection(tab);
                                                    }
                                                    let selected_row_indices =
                                                        preview_selected_row_indices(tab, row_index);
                                                    let selected_count = selected_row_indices.len();
                                                    if ui.button("添加记录").clicked() {
                                                        tab.pending_insert_row =
                                                            Some(create_empty_insert_row(&editable_columns));
                                                        tab.editing_cell = editable_columns.first().map(|first| {
                                                            TableCellEditState {
                                                                target: TableEditTarget::PendingInsert,
                                                                column: first.clone(),
                                                                value: String::new(),
                                                                is_null: false,
                                                                focus_requested: true,
                                                            }
                                                        });
                                                        ui.close();
                                                    }
                                                    if ui.button("设置为空白字符串").clicked() {
                                                        action = TabUiAction::SaveActiveTableCellEdit {
                                                            row_index,
                                                            column: column.clone(),
                                                            value: String::new(),
                                                            is_null: false,
                                                        };
                                                        ui.close();
                                                    }
                                                    if ui.button("设置为 NULL").clicked() {
                                                        action = TabUiAction::SaveActiveTableCellEdit {
                                                            row_index,
                                                            column: column.clone(),
                                                            value: String::new(),
                                                            is_null: true,
                                                        };
                                                        ui.close();
                                                    }
                                                    if ui
                                                        .add_enabled(
                                                            !tab.table.is_view,
                                                            egui::Button::new(if selected_count > 1 {
                                                                format!("删除选中 {} 条记录", selected_count)
                                                            } else {
                                                                "删除记录".into()
                                                            }),
                                                        )
                                                        .clicked()
                                                    {
                                                        action = TabUiAction::DeleteActiveTableRows(
                                                            selected_row_indices.clone(),
                                                        );
                                                        ui.close();
                                                    }
                                                    let copy_tsv_label = if selected_count > 1 {
                                                        format!("复制选中 {} 条数据", selected_count)
                                                    } else {
                                                        "复制数据".into()
                                                    };
                                                    if ui.button(copy_tsv_label).clicked() {
                                                        action =
                                                            TabUiAction::CopyActiveTableRowsAsTsv(
                                                                selected_row_indices.clone(),
                                                            );
                                                        ui.close();
                                                    }
                                                    let copy_label = if selected_count > 1 {
                                                        format!("复制选中 {} 条为 INSERT", selected_count)
                                                    } else {
                                                        "复制为 INSERT 语句".into()
                                                    };
                                                    if ui.button(copy_label).clicked() {
                                                        action =
                                                            TabUiAction::CopyActiveTableRowsAsInsert(
                                                                selected_row_indices,
                                                            );
                                                        ui.close();
                                                    }
                                                });
                                            });
                                        }
                                    });
                                }

                                if let Some(pending_row) = tab.pending_insert_row.as_mut() {
                                    body.row(30.0, |mut row_ui| {
                                        for column in &columns {
                                            row_ui.col(|ui| {
                                                let fill =
                                                    table_row_fill(&palette, row_count, false, true);
                                                let is_editing = matches!(
                                                    tab.editing_cell.as_ref(),
                                                    Some(edit)
                                                        if edit.target == TableEditTarget::PendingInsert
                                                            && edit.column == *column
                                                );
                                                let response = if is_editing {
                                                    let edit =
                                                        tab.editing_cell.as_mut().expect("edit state");
                                                    render_table_editor_cell(
                                                        ui, &palette, fill, edit, true, false,
                                                    )
                                                } else {
                                                    render_table_body_interactive_cell(
                                                        ui,
                                                        &palette,
                                                        fill,
                                                        pending_row
                                                            .get(column)
                                                            .unwrap_or(&QueryCellValue::Null),
                                                        false,
                                                        false,
                                                        false,
                                                    )
                                                };
                                                if !is_editing && response.clicked() {
                                                    let cell_value = pending_row
                                                        .get(column)
                                                        .cloned()
                                                        .unwrap_or(QueryCellValue::Null);
                                                    tab.editing_cell = Some(TableCellEditState {
                                                        target: TableEditTarget::PendingInsert,
                                                        column: column.clone(),
                                                        value: cell_value
                                                            .as_text()
                                                            .unwrap_or_default()
                                                            .to_string(),
                                                        is_null: cell_value.is_null(),
                                                        focus_requested: true,
                                                    });
                                                    ui.ctx().request_repaint();
                                                }
                                                if is_editing && response.changed() {
                                                    let edit =
                                                        tab.editing_cell.as_ref().expect("edit state");
                                                    pending_row.insert(
                                                        column.clone(),
                                                        if edit.is_null {
                                                            QueryCellValue::Null
                                                        } else {
                                                            QueryCellValue::Text(edit.value.clone())
                                                        },
                                                    );
                                                }
                                                if is_editing {
                                                    let enter_pressed = ui.ctx().input(|input| {
                                                        input.key_pressed(egui::Key::Enter)
                                                    });
                                                    if enter_pressed {
                                                        if let Some(edit) = tab.editing_cell.take() {
                                                            pending_row.insert(
                                                                column.clone(),
                                                                if edit.is_null {
                                                                    QueryCellValue::Null
                                                                } else {
                                                                    QueryCellValue::Text(edit.value)
                                                                },
                                                            );
                                                        }
                                                    } else if response.lost_focus() {
                                                        if let Some(edit) = tab.editing_cell.take() {
                                                            pending_row.insert(
                                                                column.clone(),
                                                                if edit.is_null {
                                                                    QueryCellValue::Null
                                                                } else {
                                                                    QueryCellValue::Text(edit.value)
                                                                },
                                                            );
                                                        }
                                                    }
                                                }
                                                response.context_menu(|ui| {
                                                    if ui.button("保存新增").clicked() {
                                                        action = TabUiAction::SavePendingInsertRow;
                                                        ui.close();
                                                    }
                                                    if ui.button("设置为空白字符串").clicked() {
                                                        pending_row.insert(
                                                            column.clone(),
                                                            QueryCellValue::Text(String::new()),
                                                        );
                                                        if let Some(edit) = tab.editing_cell.as_mut() {
                                                            if edit.target
                                                                == TableEditTarget::PendingInsert
                                                                && edit.column == *column
                                                            {
                                                                edit.value.clear();
                                                                edit.is_null = false;
                                                                edit.focus_requested = true;
                                                            }
                                                        }
                                                        ui.close();
                                                    }
                                                    if ui.button("设置为 NULL").clicked() {
                                                        pending_row.insert(
                                                            column.clone(),
                                                            QueryCellValue::Null,
                                                        );
                                                        if let Some(edit) = tab.editing_cell.as_mut() {
                                                            if edit.target
                                                                == TableEditTarget::PendingInsert
                                                                && edit.column == *column
                                                            {
                                                                edit.value.clear();
                                                                edit.is_null = true;
                                                                edit.focus_requested = true;
                                                            }
                                                        }
                                                        ui.close();
                                                    }
                                                    if ui.button("取消新增").clicked() {
                                                        should_cancel_pending_insert = true;
                                                        ui.close();
                                                    }
                                                });
                                            });
                                        }
                                    });
                                }
                            }
                        });
                });
        });

    if let Some((column, choice)) = selected_sort {
        match choice {
            TableHeaderSortChoice::Clear => {
                clear_table_sort_state(&mut tab.preview_sort);
            }
            TableHeaderSortChoice::Ascending | TableHeaderSortChoice::Descending => {
                set_table_sort_state(
                    &mut tab.preview_sort,
                    &column,
                    matches!(choice, TableHeaderSortChoice::Descending),
                );
            }
        }
        return TabUiAction::RefreshActiveTable {
            reload_definition: false,
        };
    }
    if should_cancel_pending_insert {
        tab.pending_insert_row = None;
        tab.editing_cell = None;
    }
    action
}

fn render_table_body_interactive_cell(
    ui: &mut egui::Ui,
    palette: &MacUiPalette,
    fill: Color32,
    value: &QueryCellValue,
    weak: bool,
    selected: bool,
    column_selected: bool,
) -> egui::Response {
    let display = query_cell_display_text(value, weak);
    let display_color = display.color(palette);
    let rect = ui.max_rect();
    let fill = if column_selected {
        blend_color(fill, palette.selection_bg, 0.12)
    } else {
        fill
    };
    let response = ui.allocate_rect(rect, egui::Sense::click());
    ui.painter()
        .rect_filled(table_cell_fill_rect(rect, selected), 0.0, fill);
    let (vertical_grid, horizontal_grid) = table_grid_colors(palette, fill, selected);
    paint_table_grid_lines(
        ui,
        rect,
        vertical_grid,
        horizontal_grid,
    );
    let clipped_rect = table_cell_content_rect(rect);
    ui.painter().with_clip_rect(clipped_rect).text(
        match display.align {
            TableCellAlign::Left => egui::pos2(clipped_rect.left(), rect.center().y),
            TableCellAlign::Center => clipped_rect.center(),
            TableCellAlign::Right => egui::pos2(clipped_rect.right(), rect.center().y),
        },
        match display.align {
            TableCellAlign::Left => Align2::LEFT_CENTER,
            TableCellAlign::Center => Align2::CENTER_CENTER,
            TableCellAlign::Right => Align2::RIGHT_CENTER,
        },
        truncate_ui_label(&display.text, 36),
        FontId::new(
            12.0,
            if display.monospace {
                FontFamily::Monospace
            } else {
                FontFamily::Proportional
            },
        ),
        display_color,
    );
    let hover_text = match value {
        QueryCellValue::Null => "(NULL)".to_string(),
        QueryCellValue::Text(text) if text.is_empty() => String::new(),
        QueryCellValue::Text(text) => text.clone(),
    };
    let response = response.on_hover_text(hover_text);
    response.on_hover_cursor(egui::CursorIcon::Text)
}

fn render_table_editor_cell(
    ui: &mut egui::Ui,
    palette: &MacUiPalette,
    fill: Color32,
    edit: &mut TableCellEditState,
    pending_insert: bool,
    selected: bool,
) -> egui::Response {
    let editor_fill = table_active_cell_fill(palette, fill, pending_insert, selected);
    ui.painter()
        .rect_filled(table_cell_fill_rect(ui.max_rect(), selected), 0.0, editor_fill);
    let mut inner = egui::Frame::new()
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, palette.selection_stroke))
        .inner_margin(egui::Margin::ZERO)
        .show(ui, |ui| {
            ui.set_min_height(28.0);
            TextEdit::singleline(&mut edit.value)
                .frame(false)
                .margin(egui::Margin::symmetric(4, 1))
                .desired_width(ui.available_width().max(24.0))
                .show(ui)
        });
    if inner.inner.response.changed() {
        edit.is_null = false;
    }
    if edit.focus_requested {
        inner.inner.response.request_focus();
        let cursor = egui::text::CCursor::new(edit.value.chars().count());
        inner
            .inner
            .state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
        inner.inner.state.store(ui.ctx(), inner.inner.response.id);
        if matches!(edit.target, TableEditTarget::PendingInsert) {
            ui.scroll_to_rect(inner.response.rect, Some(egui::Align::Center));
        }
        edit.focus_requested = false;
    }
    let (vertical_grid, horizontal_grid) = if pending_insert {
        (
            subtle_grid_color(blend_color(palette.selection_stroke, fill, 0.54), 56),
            subtle_grid_color(blend_color(palette.selection_stroke, fill, 0.44), 78),
        )
    } else {
        table_grid_colors(palette, editor_fill, selected)
    };
    paint_table_grid_lines(ui, inner.response.rect, vertical_grid, horizontal_grid);
    inner.inner.response.on_hover_cursor(egui::CursorIcon::Text)
}

fn table_row_fill(
    palette: &MacUiPalette,
    row_index: usize,
    selected: bool,
    pending_insert: bool,
) -> Color32 {
    let base_fill = if row_index % 2 == 0 {
        palette.card_bg
    } else {
        palette.table_alt_bg
    };
    if pending_insert {
        return Color32::from_rgba_premultiplied(
            palette.search_bg.r(),
            palette.search_bg.g(),
            palette.search_bg.b(),
            250,
        );
    }
    if selected {
        return blend_color(palette.selection_bg, base_fill, 0.18);
    }
    base_fill
}

fn table_row_is_selected(tab: &TableTabState, row_index: usize) -> bool {
    tab.selected_preview_rows.contains(&row_index)
        || tab.selected_preview_row == Some(row_index)
}

fn normalize_preview_selection(tab: &mut TableTabState) {
    if let Some(row_index) = tab.selected_preview_row {
        tab.selected_preview_rows.insert(row_index);
    }
    if tab.selected_preview_rows.is_empty() {
        tab.selected_preview_row = None;
        tab.selection_anchor_row = None;
        return;
    }
    if tab
        .selected_preview_row
        .is_none_or(|row_index| !tab.selected_preview_rows.contains(&row_index))
    {
        tab.selected_preview_row = tab.selected_preview_rows.iter().next_back().copied();
    }
    if tab.selection_anchor_row.is_none() {
        tab.selection_anchor_row = tab.selected_preview_row;
    }
}

fn set_single_preview_selection(tab: &mut TableTabState, row_index: usize) {
    tab.selected_preview_rows.clear();
    tab.selected_preview_rows.insert(row_index);
    tab.selected_preview_row = Some(row_index);
    tab.selection_anchor_row = Some(row_index);
}

fn toggle_preview_selection(tab: &mut TableTabState, row_index: usize) {
    if !tab.selected_preview_rows.remove(&row_index) {
        tab.selected_preview_rows.insert(row_index);
    }
    tab.selected_preview_row = if tab.selected_preview_rows.contains(&row_index) {
        Some(row_index)
    } else {
        tab.selected_preview_rows.iter().next_back().copied()
    };
    tab.selection_anchor_row = Some(row_index);
    normalize_preview_selection(tab);
}

fn extend_preview_selection(tab: &mut TableTabState, row_index: usize) {
    let anchor = tab
        .selection_anchor_row
        .or(tab.selected_preview_row)
        .unwrap_or(row_index);
    tab.selected_preview_rows.clear();
    for index in anchor.min(row_index)..=anchor.max(row_index) {
        tab.selected_preview_rows.insert(index);
    }
    tab.selected_preview_row = Some(row_index);
    tab.selection_anchor_row = Some(anchor);
    normalize_preview_selection(tab);
}

fn preview_selected_row_indices(tab: &TableTabState, fallback_row_index: usize) -> Vec<usize> {
    if tab.selected_preview_rows.is_empty() {
        vec![tab.selected_preview_row.unwrap_or(fallback_row_index)]
    } else {
        tab.selected_preview_rows.iter().copied().collect()
    }
}

fn table_editable_columns(tab: &TableTabState) -> Vec<String> {
    tab.definition
        .as_ref()
        .map(|definition| {
            definition
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>()
        })
        .filter(|columns| !columns.is_empty())
        .or_else(|| {
            tab.preview
                .as_ref()
                .map(|preview| preview.columns.clone())
                .filter(|columns| !columns.is_empty())
        })
        .unwrap_or_default()
}

fn create_empty_insert_row(columns: &[String]) -> BTreeMap<String, QueryCellValue> {
    columns
        .iter()
        .cloned()
        .map(|column| (column, QueryCellValue::Null))
        .collect()
}

fn table_active_cell_fill(
    palette: &MacUiPalette,
    base_fill: Color32,
    pending_insert: bool,
    selected: bool,
) -> Color32 {
    if pending_insert {
        Color32::from_rgba_premultiplied(
            palette.selection_bg.r(),
            palette.selection_bg.g(),
            palette.selection_bg.b(),
            110,
        )
    } else if selected {
        blend_color(palette.selection_bg, base_fill, 0.08)
    } else {
        Color32::from_rgba_premultiplied(
            base_fill.r().saturating_add(8),
            base_fill.g().saturating_add(12),
            base_fill.b().saturating_add(18),
            255,
        )
    }
}

fn table_grid_colors(
    palette: &MacUiPalette,
    fill: Color32,
    selected: bool,
) -> (Color32, Color32) {
    if selected {
        (
            Color32::TRANSPARENT,
            subtle_grid_color(blend_color(palette.selection_stroke, fill, 0.84), 72),
        )
    } else {
        (
            subtle_grid_color(palette.table_grid, 26),
            subtle_grid_color(palette.table_grid, 40),
        )
    }
}

fn table_cell_fill_rect(rect: egui::Rect, selected: bool) -> egui::Rect {
    if selected {
        rect.expand2(Vec2::new(1.0, 0.0))
    } else {
        rect
    }
}

fn table_cell_content_rect(rect: egui::Rect) -> egui::Rect {
    rect.shrink2(Vec2::new(4.0, 1.0))
}

fn render_definition_sql_view(ui: &mut egui::Ui, title: &str, create_sql: &str) {
    let palette = mac_ui_palette(ui.visuals());
    let editor = definition_editor_palette(ui.visuals());
    let code_font_size = 13.0;
    let line_number_font_size = 11.0;
    let formatted_sql = format_definition_sql(create_sql);
    let line_count = formatted_sql.lines().count().max(1);
    let viewport_width = ui.available_width().max(0.0);
    let viewport_height = ui.available_height().max(220.0);

    egui::Frame::new()
        .fill(palette.card_bg)
        .stroke(Stroke::new(1.0, palette.soft_border))
        .show(ui, |ui| {
            ui.set_width(viewport_width);
            ui.set_min_height(viewport_height);

            egui::Frame::new()
                .fill(palette.toolbar_bg)
                .stroke(Stroke::new(1.0, palette.border))
                .inner_margin(egui::Margin::symmetric(8, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.small(RichText::new("DDL").strong().color(palette.selection_text));
                        ui.separator();
                        ui.small(RichText::new(title).color(palette.weak_text));
                    });
                });

            ui.add_space(8.0);

            egui::Frame::new()
                .fill(editor.panel_bg)
                .stroke(Stroke::new(1.0, palette.soft_border))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .id_salt(format!("table-ddl-{}", title))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let row_height = 18.0;
                            StripBuilder::new(ui)
                                .size(Size::exact(42.0))
                                .size(Size::remainder())
                                .horizontal(|mut strip| {
                                    strip.cell(|ui| {
                                        let rect = ui.max_rect();
                                        let painter = ui.painter();
                                        painter.rect_filled(rect, 0.0, editor.gutter_bg);

                                        let text_x = rect.right() - 8.0;
                                        let mut y = rect.top() + 10.0;
                                        for row in 0..line_count {
                                            painter.text(
                                                egui::pos2(text_x, y),
                                                Align2::RIGHT_TOP,
                                                (row + 1).to_string(),
                                                FontId::new(
                                                    line_number_font_size,
                                                    FontFamily::Monospace,
                                                ),
                                                editor.line_number,
                                            );
                                            y += row_height;
                                        }
                                        ui.allocate_rect(rect, egui::Sense::hover());
                                    });

                                    strip.cell(|ui| {
                                        egui::Frame::new()
                                            .fill(editor.panel_bg)
                                            .inner_margin(egui::Margin::symmetric(12, 10))
                                            .show(ui, |ui| {
                                                ui.set_min_height(
                                                    (line_count as f32 * row_height).max(
                                                        ui.available_height(),
                                                    ),
                                                );
                                                ui.label(sql_highlight_job_with_font_size(
                                                    &formatted_sql,
                                                    ui.visuals(),
                                                    code_font_size,
                                                ));
                                            });
                                    });
                                });
                        });
                });
        });
}

fn format_definition_sql(sql: &str) -> String {
    ensure_sql_statement_semicolon(
        &format_create_table_ddl(sql).unwrap_or_else(|| simple_format_sql(sql)),
    )
}

fn ensure_sql_statement_semicolon(sql: &str) -> String {
    let trimmed = sql.trim_end();
    if trimmed.is_empty() || trimmed.ends_with(';') {
        return trimmed.to_string();
    }
    format!("{trimmed};")
}

fn format_create_table_ddl(sql: &str) -> Option<String> {
    let trimmed = sql.trim();
    if !trimmed.to_ascii_uppercase().starts_with("CREATE TABLE") {
        return None;
    }

    let open_index = find_top_level_char(trimmed, '(')?;
    let close_index = find_matching_paren(trimmed, open_index)?;
    let header = trimmed[..open_index].trim();
    let body = trimmed[open_index + 1..close_index].trim();
    let suffix = trimmed[close_index + 1..].trim();

    let items = split_top_level_csv(body)
        .into_iter()
        .map(|item| normalize_ddl_item(&item))
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if items.is_empty() {
        return None;
    }

    let mut lines = Vec::with_capacity(items.len() + 2);
    lines.push(format!("{header} ("));
    lines.extend(format_ddl_items(&items));
    if !suffix.is_empty() {
        lines.push(format!(") {}", format_suffix_clause(suffix)));
    } else {
        lines.push(")".to_string());
    }
    Some(lines.join("\n"))
}

fn find_top_level_char(sql: &str, target: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escape = false;

    for (index, ch) in sql.char_indices() {
        if let Some(active_quote) = quote {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' => {
                if ch == target && depth == 0 {
                    return Some(index);
                }
                depth += 1;
            }
            ')' => depth = depth.saturating_sub(1),
            _ if ch == target && depth == 0 => return Some(index),
            _ => {}
        }
    }

    None
}

fn find_matching_paren(sql: &str, open_index: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escape = false;

    for (index, ch) in sql[open_index..].char_indices() {
        let absolute_index = open_index + index;
        if let Some(active_quote) = quote {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(absolute_index);
                }
            }
            _ => {}
        }
    }

    None
}

fn split_top_level_csv(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote = None;
    let mut escape = false;

    for ch in body.chars() {
        if let Some(active_quote) = quote {
            current.push(ch);
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' | '`' => {
                quote = Some(ch);
                current.push(ch);
            }
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let item = current.trim();
                if !item.is_empty() {
                    items.push(item.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    let tail = current.trim();
    if !tail.is_empty() {
        items.push(tail.to_string());
    }
    items
}

fn normalize_ddl_item(item: &str) -> String {
    item.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_suffix_clause(suffix: &str) -> String {
    suffix.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_ddl_items(items: &[String]) -> Vec<String> {
    let parsed_items = items
        .iter()
        .map(|item| parse_ddl_item(item))
        .collect::<Vec<_>>();
    let item_count = parsed_items.len();

    parsed_items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let suffix = if index == item_count.saturating_sub(1) {
                ""
            } else {
                ","
            };
            match item {
                ParsedDdlItem::Column {
                    name,
                    data_type,
                    modifiers,
                } => {
                    let mut line = format!("  {name} {data_type}");
                    if !modifiers.is_empty() {
                        line.push(' ');
                        line.push_str(&modifiers);
                    }
                    line.push_str(suffix);
                    line
                }
                ParsedDdlItem::Constraint(value) => format!("  {value}{suffix}"),
            }
        })
        .collect()
}

fn parse_ddl_item(item: &str) -> ParsedDdlItem {
    if is_ddl_constraint_item(item) {
        return ParsedDdlItem::Constraint(item.to_string());
    }

    let tokens = item.split(' ').collect::<Vec<_>>();
    if tokens.len() < 2 {
        return ParsedDdlItem::Constraint(item.to_string());
    }

    let name = tokens[0].to_string();
    let mut type_parts = Vec::new();
    let mut rest_index = tokens.len();
    for (index, token) in tokens.iter().enumerate().skip(1) {
        if is_ddl_modifier_token(token) {
            rest_index = index;
            break;
        }
        type_parts.push(*token);
    }

    if type_parts.is_empty() {
        return ParsedDdlItem::Constraint(item.to_string());
    }

    ParsedDdlItem::Column {
        name,
        data_type: type_parts.join(" "),
        modifiers: tokens[rest_index..].join(" "),
    }
}

fn is_ddl_constraint_item(item: &str) -> bool {
    let upper = item.to_ascii_uppercase();
    upper.starts_with("PRIMARY KEY")
        || upper.starts_with("UNIQUE KEY")
        || upper.starts_with("KEY ")
        || upper.starts_with("INDEX ")
        || upper.starts_with("CONSTRAINT ")
        || upper.starts_with("FOREIGN KEY")
        || upper.starts_with("CHECK ")
}

fn is_ddl_modifier_token(token: &str) -> bool {
    matches!(
        token.to_ascii_uppercase().as_str(),
        "NOT"
            | "NULL"
            | "DEFAULT"
            | "COMMENT"
            | "AUTO_INCREMENT"
            | "PRIMARY"
            | "UNIQUE"
            | "KEY"
            | "REFERENCES"
            | "COLLATE"
            | "CHARACTER"
            | "GENERATED"
            | "AS"
            | "ON"
            | "USING"
            | "CHECK"
            | "CONSTRAINT"
    )
}

enum ParsedDdlItem {
    Column {
        name: String,
        data_type: String,
        modifiers: String,
    },
    Constraint(String),
}

fn render_table_structure_grid(ui: &mut egui::Ui, definition: &TableDefinition) {
    let palette = mac_ui_palette(ui.visuals());
    let viewport_width = ui.available_width().max(0.0);
    let viewport_height = ui.available_height().max(180.0);

    egui::Frame::new()
        .fill(palette.card_bg)
        .stroke(Stroke::new(1.0, palette.soft_border))
        .show(ui, |ui| {
            ui.set_width(viewport_width);
            ui.set_min_height(viewport_height);
            egui::ScrollArea::both()
                .id_salt(format!("table-structure-grid-{}", definition.columns.len()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .resizable(true)
                        .column(egui_extras::Column::initial(180.0).at_least(120.0))
                        .column(egui_extras::Column::initial(160.0).at_least(100.0))
                        .column(egui_extras::Column::initial(90.0).at_least(70.0))
                        .column(egui_extras::Column::initial(90.0).at_least(70.0))
                        .header(30.0, |mut header| {
                            for title in ["字段名", "类型", "可空", "主键"] {
                                header.col(|ui| {
                                    let (_, _) = table_header_cell(ui, &palette, title, false, None, false);
                                });
                            }
                        })
                        .body(|mut body| {
                            for (index, column) in definition.columns.iter().enumerate() {
                                let fill = if index % 2 == 0 {
                                    palette.card_bg
                                } else {
                                    palette.table_alt_bg
                                };
                                body.row(28.0, |mut row| {
                                    row.col(|ui| {
                                        table_text_cell(ui, &palette, fill, &column.name, false);
                                    });
                                    row.col(|ui| {
                                        table_text_cell(
                                            ui,
                                            &palette,
                                            fill,
                                            &column.data_type,
                                            false,
                                        );
                                    });
                                    row.col(|ui| {
                                        table_status_badge_cell(
                                            ui,
                                            &palette,
                                            fill,
                                            if column.nullable { "YES" } else { "NO" },
                                            column.nullable,
                                        );
                                    });
                                    row.col(|ui| {
                                        table_status_badge_cell(
                                            ui,
                                            &palette,
                                            fill,
                                            if column.primary_key { "PK" } else { "" },
                                            column.primary_key,
                                        );
                                    });
                                });
                            }
                        });
                });
        });
}

fn table_header_cell(
    ui: &mut egui::Ui,
    palette: &MacUiPalette,
    text: &str,
    sortable: bool,
    sort_state: Option<bool>,
    selected: bool,
) -> (Option<TableHeaderSortChoice>, bool) {
    let mut sort_choice = None;
    let header_bg = if selected {
        blend_color(palette.table_header_bg, palette.selection_bg, 0.35)
    } else {
        palette.table_header_bg
    };
    let inner = egui::Frame::new()
        .fill(header_bg)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_min_height(28.0);
            let content_rect = ui.max_rect();
            ui.with_layout(
                egui::Layout::top_down(egui::Align::Center)
                    .with_cross_align(egui::Align::Min),
                |ui| {
                    ui.set_min_size(egui::vec2(content_rect.width().max(20.0), content_rect.height()));
                    let label_text = match sort_state {
                        Some(false) => RichText::new(format!("{} ▲", text))
                            .size(12.5)
                            .color(palette.selection_text)
                            .strong(),
                        Some(true) => RichText::new(format!("{} ▼", text))
                            .size(12.5)
                            .color(palette.selection_text)
                            .strong(),
                        None => RichText::new(text)
                            .size(12.5)
                            .color(palette.text)
                            .strong(),
                    };
                    ui.add(
                        egui::Label::new(label_text)
                            .selectable(true),
                    )
                },
            );
        });
    let mut column_clicked = false;
    if sortable {
        let cell_rect = inner.response.rect;
        let cell_response = ui.interact(
            cell_rect,
            ui.next_auto_id(),
            egui::Sense::click(),
        );
        if cell_response.clicked() {
            column_clicked = true;
        }
        cell_response.context_menu(|ui| {
            ui.set_min_width(120.0);
            ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
            if ui
                .selectable_label(sort_state == Some(false), "▲ 升序")
                .clicked()
            {
                sort_choice = Some(TableHeaderSortChoice::Ascending);
                ui.close();
            }
            if ui
                .selectable_label(sort_state == Some(true), "▼ 降序")
                .clicked()
            {
                sort_choice = Some(TableHeaderSortChoice::Descending);
                ui.close();
            }
            if sort_state.is_some() {
                ui.separator();
                if ui.selectable_label(false, "清除排序").clicked() {
                    sort_choice = Some(TableHeaderSortChoice::Clear);
                    ui.close();
                }
            }
        });
    }
    paint_table_grid_lines(
        ui,
        inner.response.rect,
        subtle_grid_color(palette.table_grid, 40),
        subtle_grid_color(palette.table_grid, 58),
    );
    (sort_choice, column_clicked)
}

fn table_body_cell(
    ui: &mut egui::Ui,
    palette: &MacUiPalette,
    fill: Color32,
    value: &QueryCellValue,
    weak: bool,
    column_selected: bool,
) {
    let display = query_cell_display_text(value, weak);
    let display_color = display.color(palette);
    let rect = ui.max_rect();
    let fill = if column_selected {
        blend_color(fill, palette.selection_bg, 0.12)
    } else {
        fill
    };
    let response = ui.allocate_rect(rect, egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, fill);
    paint_table_grid_lines(
        ui,
        rect,
        subtle_grid_color(palette.table_grid, 26),
        subtle_grid_color(palette.table_grid, 40),
    );
    let clipped_rect = table_cell_content_rect(rect);
    ui.painter().with_clip_rect(clipped_rect).text(
        match display.align {
            TableCellAlign::Left => egui::pos2(clipped_rect.left(), rect.center().y),
            TableCellAlign::Center => clipped_rect.center(),
            TableCellAlign::Right => egui::pos2(clipped_rect.right(), rect.center().y),
        },
        match display.align {
            TableCellAlign::Left => Align2::LEFT_CENTER,
            TableCellAlign::Center => Align2::CENTER_CENTER,
            TableCellAlign::Right => Align2::RIGHT_CENTER,
        },
        truncate_ui_label(&display.text, 36),
        FontId::new(
            12.0,
            if display.monospace {
                FontFamily::Monospace
            } else {
                FontFamily::Proportional
            },
        ),
        display_color,
    );
    let hover_text = match value {
        QueryCellValue::Null => "(NULL)".to_string(),
        QueryCellValue::Text(text) if text.is_empty() => String::new(),
        QueryCellValue::Text(text) => text.clone(),
    };
    let _ = response.on_hover_text(hover_text);
}

fn table_text_cell(
    ui: &mut egui::Ui,
    palette: &MacUiPalette,
    fill: Color32,
    text: &str,
    weak: bool,
) {
    let display = table_display_text(text, weak);
    let display_color = display.color(palette);
    let rect = ui.max_rect();
    let response = ui.allocate_rect(rect, egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, fill);
    paint_table_grid_lines(
        ui,
        rect,
        subtle_grid_color(palette.table_grid, 26),
        subtle_grid_color(palette.table_grid, 40),
    );
    let clipped_rect = table_cell_content_rect(rect);
    ui.painter().with_clip_rect(clipped_rect).text(
        match display.align {
            TableCellAlign::Left => egui::pos2(clipped_rect.left(), rect.center().y),
            TableCellAlign::Center => clipped_rect.center(),
            TableCellAlign::Right => egui::pos2(clipped_rect.right(), rect.center().y),
        },
        match display.align {
            TableCellAlign::Left => Align2::LEFT_CENTER,
            TableCellAlign::Center => Align2::CENTER_CENTER,
            TableCellAlign::Right => Align2::RIGHT_CENTER,
        },
        truncate_ui_label(&display.text, 36),
        FontId::new(
            12.0,
            if display.monospace {
                FontFamily::Monospace
            } else {
                FontFamily::Proportional
            },
        ),
        display_color,
    );
    let _ = response.on_hover_text(if text.is_empty() { "无" } else { text });
}

fn table_status_badge_cell(
    ui: &mut egui::Ui,
    palette: &MacUiPalette,
    fill: Color32,
    text: &str,
    active: bool,
) {
    let desired_size = Vec2::new(ui.available_width().max(24.0), 28.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, fill);
    paint_table_grid_lines(
        ui,
        rect,
        subtle_grid_color(palette.table_grid, 26),
        subtle_grid_color(palette.table_grid, 40),
    );

    if !text.is_empty() {
        let badge_fill = if active {
            Color32::from_rgba_premultiplied(
                palette.selection_bg.r(),
                palette.selection_bg.g(),
                palette.selection_bg.b(),
                180,
            )
        } else {
            Color32::from_rgba_premultiplied(
                palette.search_bg.r(),
                palette.search_bg.g(),
                palette.search_bg.b(),
                220,
            )
        };
        let badge_stroke = if active {
            palette.selection_stroke
        } else {
            palette.soft_border
        };
        let badge_rect = egui::Rect::from_center_size(rect.center(), Vec2::new(42.0, 18.0));
        ui.painter().rect(
            badge_rect,
            9.0,
            badge_fill,
            Stroke::new(1.0, badge_stroke),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            badge_rect.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::new(11.5, FontFamily::Monospace),
            if active {
                palette.selection_text
            } else {
                palette.weak_text
            },
        );
    }

    let _ = response.on_hover_text(if text.is_empty() { "无" } else { text });
}

fn subtle_grid_color(base: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), alpha)
}

fn paint_table_grid_lines(
    ui: &egui::Ui,
    rect: egui::Rect,
    vertical: Color32,
    horizontal: Color32,
) {
    if vertical != Color32::TRANSPARENT {
        ui.painter().line_segment(
            [
                egui::pos2(rect.right() - 0.5, rect.top()),
                egui::pos2(rect.right() - 0.5, rect.bottom()),
            ],
            Stroke::new(1.0, vertical),
        );
    }
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 0.5),
            egui::pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, horizontal),
    );
}

fn sort_indicator(sort_state: &TableSortState, column: &str) -> Option<bool> {
    match sort_state.column.as_deref() {
        Some(active) if active == column => Some(sort_state.descending),
        _ => None,
    }
}

fn set_table_sort_state(sort_state: &mut TableSortState, column: &str, descending: bool) {
    sort_state.column = Some(column.to_string());
    sort_state.descending = descending;
}

fn clear_table_sort_state(sort_state: &mut TableSortState) {
    sort_state.column = None;
    sort_state.descending = false;
}

fn apply_table_sort_choice(
    result: &mut QueryResult,
    sort_state: &mut TableSortState,
    column: &str,
    descending: bool,
) {
    set_table_sort_state(sort_state, column, descending);
    sort_query_result_rows(result, column, descending);
}

fn apply_saved_table_sort(result: &mut QueryResult, sort_state: &mut TableSortState) {
    let Some(column) = sort_state.column.clone() else {
        return;
    };
    if !result.columns.iter().any(|item| item == &column) {
        sort_state.column = None;
        sort_state.descending = false;
        return;
    }
    sort_query_result_rows(result, &column, sort_state.descending);
}

impl TableFilterOperator {
    const ALL: [Self; 21] = [
        Self::Eq,
        Self::NotEq,
        Self::Lt,
        Self::LtEq,
        Self::Gt,
        Self::GtEq,
        Self::Contains,
        Self::NotContains,
        Self::BeginsWith,
        Self::NotBeginsWith,
        Self::EndsWith,
        Self::NotEndsWith,
        Self::IsNull,
        Self::IsNotNull,
        Self::IsEmpty,
        Self::IsNotEmpty,
        Self::Between,
        Self::NotBetween,
        Self::InList,
        Self::NotInList,
        Self::Custom,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::NotEq => "!=",
            Self::Lt => "<",
            Self::LtEq => "<=",
            Self::Gt => ">",
            Self::GtEq => ">=",
            Self::Contains => "包含",
            Self::NotContains => "不包含",
            Self::BeginsWith => "开始是",
            Self::NotBeginsWith => "开始不是",
            Self::EndsWith => "结束是",
            Self::NotEndsWith => "结束不是",
            Self::IsNull => "是 null",
            Self::IsNotNull => "不是 null",
            Self::IsEmpty => "是空的",
            Self::IsNotEmpty => "不是空的",
            Self::Between => "介于",
            Self::NotBetween => "不介于",
            Self::InList => "在列表",
            Self::NotInList => "不在列表",
            Self::Custom => "[自定义]",
        }
    }

    fn uses_primary_value(self) -> bool {
        !matches!(
            self,
            Self::IsNull | Self::IsNotNull | Self::IsEmpty | Self::IsNotEmpty
        )
    }

    fn uses_secondary_value(self) -> bool {
        matches!(self, Self::Between | Self::NotBetween)
    }

    fn value_hint(self) -> &'static str {
        match self {
            Self::Contains | Self::NotContains => "输入匹配内容",
            Self::BeginsWith | Self::NotBeginsWith => "输入前缀",
            Self::EndsWith | Self::NotEndsWith => "输入后缀",
            Self::Between | Self::NotBetween => "输入起始值",
            Self::InList | Self::NotInList => "逗号分隔多个值",
            Self::Custom => "输入原始 SQL 条件",
            _ => "输入值",
        }
    }
}

impl Default for TableFilterState {
    fn default() -> Self {
        Self {
            clauses: vec![TableFilterClause::default()],
        }
    }
}

impl Default for TableFilterClause {
    fn default() -> Self {
        Self {
            joiner: TableFilterJoiner::And,
            column: None,
            operator: TableFilterOperator::default(),
            value: String::new(),
            second_value: String::new(),
        }
    }
}

impl TableFilterJoiner {
    const ALL: [Self; 2] = [Self::And, Self::Or];

    fn label(self) -> &'static str {
        match self {
            Self::And => "AND",
            Self::Or => "OR",
        }
    }
}

fn table_filter_columns(tab: &TableTabState) -> Vec<String> {
    tab.definition
        .as_ref()
        .map(|definition| {
            definition
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>()
        })
        .filter(|columns| !columns.is_empty())
        .or_else(|| {
            tab.preview
                .as_ref()
                .map(|preview| preview.columns.clone())
                .filter(|columns| !columns.is_empty())
        })
        .unwrap_or_default()
}

fn ensure_table_filter_column(filter: &mut TableFilterState, columns: &[String]) {
    if filter.clauses.is_empty() {
        filter.clauses.push(TableFilterClause::default());
    }
    if columns.is_empty() {
        for clause in &mut filter.clauses {
            clause.column = None;
        }
        return;
    }

    for clause in &mut filter.clauses {
        if clause
            .column
            .as_ref()
            .is_some_and(|column| columns.iter().any(|item| item == column))
        {
            continue;
        }
        clause.column = columns.first().cloned();
    }
}

fn table_filter_summary(filter: &TableFilterState) -> Option<String> {
    let summaries = filter
        .clauses
        .iter()
        .filter_map(table_filter_clause_summary)
        .collect::<Vec<_>>();
    if summaries.is_empty() {
        return None;
    }

    let mut result = vec![summaries[0].clone()];
    for (clause, summary) in filter.clauses.iter().skip(1).zip(summaries.iter().skip(1)) {
        result.push(format!("{} {summary}", clause.joiner.label()));
    }
    Some(result.join(" "))
}

fn build_table_preview_sql(
    database_kind: DatabaseKind,
    table: &TableRef,
    filter: &TableFilterState,
    sort: &TableSortState,
    limit: Option<u32>,
) -> String {
    let mut sql = format!("SELECT *\nFROM {}", qualified_table_name(database_kind, table));

    if let Some(filter_clause) = build_table_filter_clause(database_kind, filter) {
        sql.push_str("\nWHERE ");
        sql.push_str(&filter_clause);
    }

    if let Some(column) = sort.column.as_deref() {
        sql.push_str("\nORDER BY ");
        sql.push_str(&quote_identifier(database_kind, column));
        sql.push(' ');
        sql.push_str(if sort.descending { "DESC" } else { "ASC" });
    }

    if let Some(limit) = limit {
        sql.push_str(&format!("\nLIMIT {limit}"));
    }
    sql
}

fn build_table_preview_display_sql(
    database_kind: DatabaseKind,
    table: &TableRef,
    filter: &TableFilterState,
    sort: &TableSortState,
    limit: Option<u32>,
) -> String {
    let mut parts = vec![format!(
        "SELECT * FROM {}",
        qualified_table_name_display(database_kind, table)
    )];

    if let Some(filter_clause) = build_table_filter_display_clause(filter) {
        parts.push(format!("WHERE {filter_clause}"));
    }

    if let Some(column) = sort.column.as_deref() {
        parts.push(format!(
            "ORDER BY {} {}",
            column,
            if sort.descending { "DESC" } else { "ASC" }
        ));
    }

    if let Some(limit) = limit {
        parts.push(format!("LIMIT {limit}"));
    }

    parts.join(" ")
}

fn build_table_filter_clause(
    database_kind: DatabaseKind,
    filter: &TableFilterState,
) -> Option<String> {
    let clauses = filter
        .clauses
        .iter()
        .filter_map(|clause| {
            build_single_table_filter_clause(database_kind, clause).map(|sql| (clause.joiner, sql))
        })
        .collect::<Vec<_>>();
    if clauses.is_empty() {
        return None;
    }

    let mut result = vec![format!("({})", clauses[0].1)];
    for (joiner, sql) in clauses.into_iter().skip(1) {
        result.push(format!("{} ({sql})", joiner.label()));
    }
    Some(result.join(" "))
}

fn build_table_filter_display_clause(filter: &TableFilterState) -> Option<String> {
    let clauses = filter
        .clauses
        .iter()
        .filter_map(|clause| {
            build_single_table_filter_display_clause(clause).map(|sql| (clause.joiner, sql))
        })
        .collect::<Vec<_>>();
    if clauses.is_empty() {
        return None;
    }

    let mut result = vec![clauses[0].1.clone()];
    for (joiner, sql) in clauses.into_iter().skip(1) {
        result.push(format!("{} {sql}", joiner.label()));
    }
    Some(result.join(" "))
}

fn table_filter_clause_summary(filter: &TableFilterClause) -> Option<String> {
    if filter.operator == TableFilterOperator::Custom {
        let value = filter.value.trim();
        return (!value.is_empty()).then(|| format!("自定义: {value}"));
    }

    let column = filter.column.as_deref()?.trim();
    if column.is_empty() {
        return None;
    }

    if filter.operator.uses_secondary_value() {
        let left = filter.value.trim();
        let right = filter.second_value.trim();
        if left.is_empty() || right.is_empty() {
            return None;
        }
        return Some(format!("{column} {} {} ~ {}", filter.operator.label(), left, right));
    }

    if filter.operator.uses_primary_value() {
        let value = filter.value.trim();
        if value.is_empty() {
            return None;
        }
        return Some(format!("{column} {} {value}", filter.operator.label()));
    }

    Some(format!("{column} {}", filter.operator.label()))
}

fn build_single_table_filter_clause(
    database_kind: DatabaseKind,
    filter: &TableFilterClause,
) -> Option<String> {
    if filter.operator == TableFilterOperator::Custom {
        let raw = filter.value.trim();
        return (!raw.is_empty()).then(|| raw.to_string());
    }

    let column = quote_identifier(database_kind, filter.column.as_deref()?.trim());
    let text_column = cast_column_to_text(database_kind, &column);
    let primary = filter.value.trim();
    let secondary = filter.second_value.trim();

    match filter.operator {
        TableFilterOperator::Eq => (!primary.is_empty())
            .then(|| format!("{column} = {}", sql_string_literal(primary))),
        TableFilterOperator::NotEq => (!primary.is_empty())
            .then(|| format!("{column} <> {}", sql_string_literal(primary))),
        TableFilterOperator::Lt => (!primary.is_empty())
            .then(|| format!("{column} < {}", sql_string_literal(primary))),
        TableFilterOperator::LtEq => (!primary.is_empty())
            .then(|| format!("{column} <= {}", sql_string_literal(primary))),
        TableFilterOperator::Gt => (!primary.is_empty())
            .then(|| format!("{column} > {}", sql_string_literal(primary))),
        TableFilterOperator::GtEq => (!primary.is_empty())
            .then(|| format!("{column} >= {}", sql_string_literal(primary))),
        TableFilterOperator::Contains => build_like_clause(&text_column, primary, "%", "%", false),
        TableFilterOperator::NotContains => {
            build_like_clause(&text_column, primary, "%", "%", true)
        }
        TableFilterOperator::BeginsWith => build_like_clause(&text_column, primary, "", "%", false),
        TableFilterOperator::NotBeginsWith => {
            build_like_clause(&text_column, primary, "", "%", true)
        }
        TableFilterOperator::EndsWith => build_like_clause(&text_column, primary, "%", "", false),
        TableFilterOperator::NotEndsWith => {
            build_like_clause(&text_column, primary, "%", "", true)
        }
        TableFilterOperator::IsNull => Some(format!("{column} IS NULL")),
        TableFilterOperator::IsNotNull => Some(format!("{column} IS NOT NULL")),
        TableFilterOperator::IsEmpty => Some(format!("COALESCE({text_column}, '') = ''")),
        TableFilterOperator::IsNotEmpty => Some(format!("COALESCE({text_column}, '') <> ''")),
        TableFilterOperator::Between => {
            (!primary.is_empty() && !secondary.is_empty()).then(|| {
                format!(
                    "{column} BETWEEN {} AND {}",
                    sql_string_literal(primary),
                    sql_string_literal(secondary)
                )
            })
        }
        TableFilterOperator::NotBetween => {
            (!primary.is_empty() && !secondary.is_empty()).then(|| {
                format!(
                    "{column} NOT BETWEEN {} AND {}",
                    sql_string_literal(primary),
                    sql_string_literal(secondary)
                )
            })
        }
        TableFilterOperator::InList => build_list_clause(&column, primary, false),
        TableFilterOperator::NotInList => build_list_clause(&column, primary, true),
        TableFilterOperator::Custom => None,
    }
}

fn build_single_table_filter_display_clause(filter: &TableFilterClause) -> Option<String> {
    if filter.operator == TableFilterOperator::Custom {
        let raw = filter.value.trim();
        return (!raw.is_empty()).then(|| raw.to_string());
    }

    let column = filter.column.as_deref()?.trim();
    if column.is_empty() {
        return None;
    }

    let primary = filter.value.trim();
    let secondary = filter.second_value.trim();

    match filter.operator {
        TableFilterOperator::Eq => (!primary.is_empty())
            .then(|| format!("{column} = {}", sql_string_literal(primary))),
        TableFilterOperator::NotEq => (!primary.is_empty())
            .then(|| format!("{column} <> {}", sql_string_literal(primary))),
        TableFilterOperator::Lt => (!primary.is_empty())
            .then(|| format!("{column} < {}", sql_string_literal(primary))),
        TableFilterOperator::LtEq => (!primary.is_empty())
            .then(|| format!("{column} <= {}", sql_string_literal(primary))),
        TableFilterOperator::Gt => (!primary.is_empty())
            .then(|| format!("{column} > {}", sql_string_literal(primary))),
        TableFilterOperator::GtEq => (!primary.is_empty())
            .then(|| format!("{column} >= {}", sql_string_literal(primary))),
        TableFilterOperator::Contains => {
            build_simple_like_display_clause(column, primary, "%", "%", false)
        }
        TableFilterOperator::NotContains => {
            build_simple_like_display_clause(column, primary, "%", "%", true)
        }
        TableFilterOperator::BeginsWith => {
            build_simple_like_display_clause(column, primary, "", "%", false)
        }
        TableFilterOperator::NotBeginsWith => {
            build_simple_like_display_clause(column, primary, "", "%", true)
        }
        TableFilterOperator::EndsWith => {
            build_simple_like_display_clause(column, primary, "%", "", false)
        }
        TableFilterOperator::NotEndsWith => {
            build_simple_like_display_clause(column, primary, "%", "", true)
        }
        TableFilterOperator::IsNull => Some(format!("{column} IS NULL")),
        TableFilterOperator::IsNotNull => Some(format!("{column} IS NOT NULL")),
        TableFilterOperator::IsEmpty => Some(format!("{column} = ''")),
        TableFilterOperator::IsNotEmpty => Some(format!("{column} <> ''")),
        TableFilterOperator::Between => (!primary.is_empty() && !secondary.is_empty()).then(|| {
            format!(
                "{column} BETWEEN {} AND {}",
                sql_string_literal(primary),
                sql_string_literal(secondary)
            )
        }),
        TableFilterOperator::NotBetween => {
            (!primary.is_empty() && !secondary.is_empty()).then(|| {
                format!(
                    "{column} NOT BETWEEN {} AND {}",
                    sql_string_literal(primary),
                    sql_string_literal(secondary)
                )
            })
        }
        TableFilterOperator::InList => build_list_clause(column, primary, false),
        TableFilterOperator::NotInList => build_list_clause(column, primary, true),
        TableFilterOperator::Custom => None,
    }
}

fn build_like_clause(
    column_expr: &str,
    value: &str,
    prefix: &str,
    suffix: &str,
    negate: bool,
) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    let escaped = escape_like_pattern(value);
    let pattern = format!("{prefix}{escaped}{suffix}");
    Some(format!(
        "LOWER({column_expr}) {} LOWER({}) ESCAPE '\\\\'",
        if negate { "NOT LIKE" } else { "LIKE" },
        sql_string_literal(&pattern)
    ))
}

fn build_simple_like_display_clause(
    column: &str,
    value: &str,
    prefix: &str,
    suffix: &str,
    negate: bool,
) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    Some(format!(
        "{column} {} {}",
        if negate { "NOT LIKE" } else { "LIKE" },
        sql_string_literal(&format!("{prefix}{value}{suffix}"))
    ))
}

fn build_list_clause(column: &str, value: &str, negate: bool) -> Option<String> {
    let values = value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(sql_string_literal)
        .collect::<Vec<_>>();

    if values.is_empty() {
        return None;
    }

    Some(format!(
        "{column} {} ({})",
        if negate { "NOT IN" } else { "IN" },
        values.join(", ")
    ))
}

fn qualified_table_name(database_kind: DatabaseKind, table: &TableRef) -> String {
    let mut segments = Vec::new();
    match database_kind {
        DatabaseKind::MySql => {
            if let Some(database) = table.database.as_deref().filter(|value| !value.is_empty()) {
                segments.push(quote_identifier(database_kind, database));
            }
        }
        DatabaseKind::Postgres => {
            if let Some(schema) = table.schema.as_deref().filter(|value| !value.is_empty()) {
                segments.push(quote_identifier(database_kind, schema));
            }
        }
    }
    segments.push(quote_identifier(database_kind, &table.table));
    segments.join(".")
}

fn qualified_table_name_display(database_kind: DatabaseKind, table: &TableRef) -> String {
    let mut segments = Vec::new();
    match database_kind {
        DatabaseKind::MySql => {
            if let Some(database) = table.database.as_deref().filter(|value| !value.is_empty()) {
                segments.push(database.to_string());
            }
        }
        DatabaseKind::Postgres => {
            if let Some(schema) = table.schema.as_deref().filter(|value| !value.is_empty()) {
                segments.push(schema.to_string());
            }
        }
    }
    segments.push(table.table.clone());
    segments.join(".")
}

fn quote_identifier(database_kind: DatabaseKind, identifier: &str) -> String {
    match database_kind {
        DatabaseKind::MySql => format!("`{}`", identifier.replace('`', "``")),
        DatabaseKind::Postgres => format!("\"{}\"", identifier.replace('"', "\"\"")),
    }
}

fn cast_column_to_text(database_kind: DatabaseKind, column: &str) -> String {
    match database_kind {
        DatabaseKind::MySql => format!("CAST({column} AS CHAR)"),
        DatabaseKind::Postgres => format!("CAST({column} AS TEXT)"),
    }
}

fn sql_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn sql_editor_value_literal(value: &str, is_null: bool) -> String {
    if is_null || value.trim().eq_ignore_ascii_case("null") {
        "NULL".into()
    } else {
        sql_string_literal(value)
    }
}

fn build_insert_sql_for_existing_row(
    database_kind: DatabaseKind,
    table: &TableRef,
    columns: &[String],
    row: &BTreeMap<String, QueryCellValue>,
) -> String {
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(database_kind, column))
        .collect::<Vec<_>>();
    let values = columns
        .iter()
        .map(|column| {
            row.get(column)
                .map(query_cell_sql_literal)
                .unwrap_or_else(|| "NULL".into())
        })
        .collect::<Vec<_>>();
    format!(
        "INSERT INTO {} ({})\nVALUES ({});",
        qualified_table_name(database_kind, table),
        quoted_columns.join(", "),
        values.join(", ")
    )
}

fn build_insert_sql_for_existing_rows(
    database_kind: DatabaseKind,
    table: &TableRef,
    columns: &[String],
    rows: &[BTreeMap<String, QueryCellValue>],
) -> String {
    if rows.len() == 1 {
        return build_insert_sql_for_existing_row(database_kind, table, columns, &rows[0]);
    }
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(database_kind, column))
        .collect::<Vec<_>>();
    let value_rows = rows
        .iter()
        .map(|row| {
            let values = columns
                .iter()
                .map(|column| {
                    row.get(column)
                        .map(query_cell_sql_literal)
                        .unwrap_or_else(|| "NULL".into())
                })
                .collect::<Vec<_>>();
            format!("({})", values.join(", "))
        })
        .collect::<Vec<_>>();
    format!(
        "INSERT INTO {} ({})\nVALUES\n  {};",
        qualified_table_name(database_kind, table),
        quoted_columns.join(", "),
        value_rows.join(",\n  ")
    )
}

fn build_insert_sql_for_pending_row(
    database_kind: DatabaseKind,
    table: &TableRef,
    columns: &[String],
    values: &BTreeMap<String, QueryCellValue>,
) -> Option<String> {
    let included = columns
        .iter()
        .filter_map(|column| {
            let value = values.get(column)?;
            match value {
                QueryCellValue::Null => None,
                QueryCellValue::Text(text) if text.is_empty() => None,
                QueryCellValue::Text(text) => {
                    Some((column.clone(), QueryCellValue::Text(text.clone())))
                }
            }
        })
        .collect::<Vec<_>>();
    if included.is_empty() {
        return None;
    }
    let quoted_columns = included
        .iter()
        .map(|(column, _)| quote_identifier(database_kind, column))
        .collect::<Vec<_>>();
    let sql_values = included
        .iter()
        .map(|(_, value)| query_cell_sql_literal(value))
        .collect::<Vec<_>>();
    Some(format!(
        "INSERT INTO {} ({})\nVALUES ({});",
        qualified_table_name(database_kind, table),
        quoted_columns.join(", "),
        sql_values.join(", ")
    ))
}

fn build_table_row_match_clause(
    database_kind: DatabaseKind,
    definition: Option<&TableDefinition>,
    row: &BTreeMap<String, QueryCellValue>,
    _table: &TableRef,
) -> Option<String> {
    let mut key_columns = definition
        .map(|definition| {
            definition
                .columns
                .iter()
                .filter(|column| column.primary_key)
                .map(|column| column.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    key_columns.retain(|column| row.contains_key(column));
    if key_columns.is_empty() {
        key_columns = row.keys().cloned().collect();
    }
    if key_columns.is_empty() {
        return None;
    }
    let key_column_count = key_columns.len();
    let has_primary_key = definition.is_some_and(|item| item.columns.iter().any(|c| c.primary_key));

    let predicates = key_columns
        .into_iter()
        .map(|column| build_table_row_value_match(database_kind, &column, row.get(&column)))
        .collect::<Option<Vec<_>>>()?;

    let mut clause = predicates.join("\n  AND ");
    if key_column_count != row.len() || has_primary_key {
        clause = format!("({clause})");
    }
    Some(clause)
}

fn build_table_row_value_match(
    database_kind: DatabaseKind,
    column: &str,
    value: Option<&QueryCellValue>,
) -> Option<String> {
    let quoted = quote_identifier(database_kind, column);
    let value = value?;
    if value.is_null() {
        return Some(format!("{quoted} IS NULL"));
    }
    if value.is_empty_text() {
        return Some(format!("COALESCE({}, '') = ''", cast_column_to_text(database_kind, &quoted)));
    }
    Some(format!(
        "{quoted} = {}",
        sql_string_literal(value.as_text().unwrap_or_default())
    ))
}

fn query_cell_sql_literal(value: &QueryCellValue) -> String {
    match value {
        QueryCellValue::Null => "NULL".into(),
        QueryCellValue::Text(text) => sql_string_literal(text),
    }
}

fn escape_like_pattern(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn sort_query_result_rows(result: &mut QueryResult, column: &str, descending: bool) {
    result.rows.sort_by(|left, right| {
        let ordering = compare_table_cell_value(left.get(column), right.get(column));
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn compare_table_cell_value(
    left: Option<&QueryCellValue>,
    right: Option<&QueryCellValue>,
) -> Ordering {
    match (left, right) {
        (None | Some(QueryCellValue::Null), None | Some(QueryCellValue::Null)) => {
            return Ordering::Equal;
        }
        (None | Some(QueryCellValue::Null), _) => return Ordering::Greater,
        (_, None | Some(QueryCellValue::Null)) => return Ordering::Less,
        (Some(QueryCellValue::Text(left)), Some(QueryCellValue::Text(right))) => {
            return compare_text_cell_value(left, right);
        }
    }
}

fn compare_text_cell_value(left: &str, right: &str) -> Ordering {
    let left = left.trim();
    let right = right.trim();

    match (left.is_empty(), right.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }

    if let (Ok(left_num), Ok(right_num)) = (left.parse::<f64>(), right.parse::<f64>()) {
        return left_num
            .partial_cmp(&right_num)
            .unwrap_or(Ordering::Equal);
    }

    let left_lower = left.to_lowercase();
    let right_lower = right.to_lowercase();
    left_lower.cmp(&right_lower).then_with(|| left.cmp(right))
}

fn estimate_query_column_widths(
    columns: &[String],
    rows: &[BTreeMap<String, QueryCellValue>],
) -> Vec<f32> {
    columns
        .iter()
        .map(|column| {
            let header_width = estimate_table_header_width(column);
            let body_width = rows
                .iter()
                .take(300)
                .filter_map(|row| row.get(column))
                .map(|value| estimate_table_text_width(&query_cell_display_text(value, false).text) + 20.0)
                .fold(0.0, f32::max);

            let body_auto_width = body_width.clamp(88.0, 260.0);
            header_width.max(body_auto_width).max(88.0)
        })
        .collect()
}

fn estimate_result_column_widths(result: &QueryResult) -> Vec<f32> {
    estimate_query_column_widths(&result.columns, &result.rows)
}

fn estimate_table_header_width(text: &str) -> f32 {
    estimate_table_text_width(text) + 44.0
}

fn estimate_table_text_width(text: &str) -> f32 {
    text.chars()
        .take(40)
        .map(|ch| match ch {
            '0'..='9' => 7.0,
            'a'..='z' | 'A'..='Z' | '_' | '-' | '.' | ':' | '/' => 7.6,
            '\u{4E00}'..='\u{9FFF}' => 13.0,
            _ if ch.is_ascii_punctuation() => 6.8,
            _ if ch.is_whitespace() => 4.5,
            _ => 9.0,
        })
        .sum()
}

#[derive(Clone, Copy)]
enum TableCellAlign {
    Left,
    Center,
    Right,
}

struct TableCellDisplay {
    text: String,
    tone: TableCellTone,
    align: TableCellAlign,
    monospace: bool,
}

#[derive(Clone, Copy)]
enum TableCellTone {
    Normal,
    Weak,
    Accent,
}

impl TableCellDisplay {
    fn color(&self, palette: &MacUiPalette) -> Color32 {
        match self.tone {
            TableCellTone::Normal => palette.text,
            TableCellTone::Weak => palette.weak_text,
            TableCellTone::Accent => palette.selection_text,
        }
    }
}

fn table_display_text(text: &str, weak: bool) -> TableCellDisplay {
    let trimmed = text.trim();
    if weak {
        return TableCellDisplay {
            text: text.to_string(),
            tone: TableCellTone::Weak,
            align: TableCellAlign::Center,
            monospace: true,
        };
    }

    if looks_like_number(trimmed) {
        return TableCellDisplay {
            text: truncate_ui_label(trimmed, 22),
            tone: TableCellTone::Normal,
            align: TableCellAlign::Right,
            monospace: true,
        };
    }

    if looks_like_json(trimmed) {
        return TableCellDisplay {
            text: truncate_ui_label(trimmed, 28),
            tone: TableCellTone::Accent,
            align: TableCellAlign::Left,
            monospace: true,
        };
    }

    if looks_like_datetime(trimmed) {
        return TableCellDisplay {
            text: truncate_ui_label(trimmed, 24),
            tone: TableCellTone::Normal,
            align: TableCellAlign::Left,
            monospace: true,
        };
    }

    TableCellDisplay {
        text: truncate_ui_label(trimmed, 30),
        tone: TableCellTone::Normal,
        align: TableCellAlign::Left,
        monospace: false,
    }
}

fn query_cell_display_text(value: &QueryCellValue, weak: bool) -> TableCellDisplay {
    match value {
        QueryCellValue::Null => TableCellDisplay {
            text: "(NULL)".into(),
            tone: TableCellTone::Weak,
            align: TableCellAlign::Center,
            monospace: false,
        },
        QueryCellValue::Text(text) if text.is_empty() => TableCellDisplay {
            text: String::new(),
            tone: TableCellTone::Normal,
            align: TableCellAlign::Left,
            monospace: false,
        },
        QueryCellValue::Text(text) => table_display_text(text, weak),
    }
}

fn looks_like_number(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | ','))
}

fn looks_like_json(text: &str) -> bool {
    (text.starts_with('{') && text.ends_with('}')) || (text.starts_with('[') && text.ends_with(']'))
}

fn looks_like_datetime(text: &str) -> bool {
    (text.contains('-') && text.contains(':'))
        || text.ends_with('Z')
        || text.contains('T') && text.chars().any(|ch| ch.is_ascii_digit())
}

#[derive(Clone, Copy)]
struct MacDialogPalette {
    window_bg: Color32,
    border: Color32,
    section_bg: Color32,
    section_border: Color32,
    input_bg: Color32,
    input_hover_bg: Color32,
    input_active_bg: Color32,
    input_border: Color32,
    title: Color32,
    subtitle: Color32,
    text: Color32,
    weak_text: Color32,
    primary_button_bg: Color32,
    primary_button_stroke: Color32,
    primary_button_text: Color32,
    secondary_button_bg: Color32,
    secondary_button_stroke: Color32,
    secondary_button_text: Color32,
}

fn mac_dialog_palette(dark_mode: bool) -> MacDialogPalette {
    if dark_mode {
        MacDialogPalette {
            window_bg: Color32::from_rgb(50, 53, 59),
            border: Color32::from_rgb(84, 89, 98),
            section_bg: Color32::from_rgb(60, 64, 71),
            section_border: Color32::from_rgb(96, 101, 110),
            input_bg: Color32::from_rgb(74, 79, 87),
            input_hover_bg: Color32::from_rgb(82, 87, 96),
            input_active_bg: Color32::from_rgb(88, 94, 103),
            input_border: Color32::from_rgb(106, 112, 123),
            title: Color32::from_rgb(248, 249, 251),
            subtitle: Color32::from_rgb(194, 199, 208),
            text: Color32::from_rgb(238, 241, 246),
            weak_text: Color32::from_rgb(202, 207, 216),
            primary_button_bg: Color32::from_rgb(10, 132, 255),
            primary_button_stroke: Color32::from_rgb(64, 157, 255),
            primary_button_text: Color32::WHITE,
            secondary_button_bg: Color32::from_rgb(90, 96, 105),
            secondary_button_stroke: Color32::from_rgb(117, 123, 133),
            secondary_button_text: Color32::from_rgb(246, 247, 249),
        }
    } else {
        MacDialogPalette {
            window_bg: Color32::from_rgb(246, 247, 249),
            border: Color32::from_rgb(217, 222, 229),
            section_bg: Color32::from_rgb(252, 252, 253),
            section_border: Color32::from_rgb(226, 229, 235),
            input_bg: Color32::from_rgb(255, 255, 255),
            input_hover_bg: Color32::from_rgb(252, 253, 255),
            input_active_bg: Color32::from_rgb(255, 255, 255),
            input_border: Color32::from_rgb(208, 214, 223),
            title: Color32::from_rgb(35, 39, 46),
            subtitle: Color32::from_rgb(103, 111, 122),
            text: Color32::from_rgb(48, 54, 64),
            weak_text: Color32::from_rgb(111, 119, 130),
            primary_button_bg: Color32::from_rgb(0, 122, 255),
            primary_button_stroke: Color32::from_rgb(0, 114, 240),
            primary_button_text: Color32::WHITE,
            secondary_button_bg: Color32::from_rgb(238, 241, 245),
            secondary_button_stroke: Color32::from_rgb(214, 220, 228),
            secondary_button_text: Color32::from_rgb(57, 63, 74),
        }
    }
}

fn app_visuals(use_dark_theme: bool) -> egui::Visuals {
    let mut visuals = if use_dark_theme {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if use_dark_theme {
        visuals.panel_fill = Color32::from_rgb(56, 59, 66);
        visuals.window_fill = Color32::from_rgb(52, 55, 61);
        visuals.extreme_bg_color = Color32::from_rgb(74, 79, 87);
        visuals.faint_bg_color = Color32::from_rgb(58, 62, 68);
        visuals.code_bg_color = Color32::from_rgb(70, 75, 82);
        visuals.override_text_color = Some(Color32::from_rgb(236, 239, 244));
        visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(88, 94, 103));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(60, 64, 70);
        visuals.widgets.noninteractive.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(88, 94, 103));
        visuals.widgets.noninteractive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(196, 201, 210));
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(74, 79, 87);
        visuals.widgets.inactive.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(105, 111, 122));
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(82, 87, 95);
        visuals.widgets.hovered.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(120, 146, 191));
        visuals.widgets.active.bg_fill = Color32::from_rgb(88, 94, 103);
        visuals.widgets.active.bg_stroke =
            Stroke::new(1.2, Color32::from_rgb(124, 153, 200));
        visuals.widgets.open.bg_fill = Color32::from_rgb(82, 87, 95);
        visuals.widgets.open.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(110, 117, 127));
        visuals.selection.bg_fill = Color32::from_rgba_premultiplied(80, 138, 205, 100);
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgba_premultiplied(140, 175, 230, 130));
    } else {
        // Light mode: use a transparent selection background so text remains readable
        visuals.selection.bg_fill = Color32::from_rgba_premultiplied(144, 209, 255, 100);
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgba_premultiplied(0, 83, 125, 130));
    }
    visuals
}

fn app_style(base_style: &egui::Style) -> egui::Style {
    let mut style = base_style.clone();
    let dark = base_style.visuals.dark_mode;
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.spacing.scroll.bar_width = 8.0;
    style.spacing.scroll.floating_width = 6.0;
    style.spacing.scroll.floating_allocated_width = 6.0;
    style.spacing.scroll.handle_min_length = 28.0;
    style.spacing.scroll.foreground_color = true;
    style.spacing.scroll.dormant_handle_opacity = if dark { 0.50 } else { 0.45 };
    style.spacing.scroll.active_handle_opacity = if dark { 0.60 } else { 0.55 };
    style.spacing.scroll.interact_handle_opacity = if dark { 0.75 } else { 0.70 };
    style
}

fn apply_mac_dialog_style(ui: &mut egui::Ui, palette: MacDialogPalette) {
    let style = ui.style_mut();
    style.visuals.override_text_color = Some(palette.text);
    style.visuals.extreme_bg_color = palette.input_bg;
    style.visuals.faint_bg_color = palette.section_bg;
    style.visuals.code_bg_color = palette.input_bg;
    style.visuals.selection.bg_fill = Color32::from_rgba_premultiplied(10, 132, 255, 80);
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(10, 132, 255));

    style.visuals.widgets.noninteractive.bg_fill = palette.section_bg;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.section_border);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.weak_text);
    style.visuals.widgets.inactive.bg_fill = palette.input_bg;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.input_border);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
    style.visuals.widgets.hovered.bg_fill = palette.input_hover_bg;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(10, 132, 255));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
    style.visuals.widgets.active.bg_fill = palette.input_active_bg;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.2, Color32::from_rgb(10, 132, 255));
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.text);
    style.visuals.widgets.open.bg_fill = palette.input_hover_bg;
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette.input_border);
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0, palette.text);
}

fn dialog_button(ui: &mut egui::Ui, label: &str, primary: bool) -> egui::Response {
    let palette = mac_dialog_palette(ui.visuals().dark_mode);
    let (fill, stroke, text) = if primary {
        (
            palette.primary_button_bg,
            Stroke::new(1.0, palette.primary_button_stroke),
            palette.primary_button_text,
        )
    } else {
        (
            palette.secondary_button_bg,
            Stroke::new(1.0, palette.secondary_button_stroke),
            palette.secondary_button_text,
        )
    };

    ui.add(
        egui::Button::new(RichText::new(label).size(13.0).strong().color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(8.0)
            .min_size(Vec2::new(92.0, 32.0)),
    )
}

fn form_grid_row(ui: &mut egui::Ui, label: &str, add_value: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(label).color(ui.visuals().weak_text_color()));
    add_value(ui);
    ui.end_row();
}

fn form_row(ui: &mut egui::Ui, label: &str, value: &mut String) {
    form_grid_row(ui, label, |ui| {
        ui.add_sized([380.0, 30.0], TextEdit::singleline(value));
    });
}

fn form_row_u16(ui: &mut egui::Ui, label: &str, value: &mut u16) {
    form_grid_row(ui, label, |ui| {
        ui.add_sized([120.0, 30.0], egui::DragValue::new(value).range(1..=65535));
    });
}

fn optional_string(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.trim().to_string())
    }
}

fn current_line_number(sql: &str, cursor_range: Option<egui::text::CCursorRange>) -> usize {
    let Some(range) = cursor_range else {
        return 1;
    };

    let cursor = range.primary.index.min(sql.chars().count());
    let mut line = 1;
    for (index, ch) in sql.chars().enumerate() {
        if index >= cursor {
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line
}

fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let next = if i + 1 < chars.len() { chars[i + 1] } else { '\0' };

        // 处理注释和引号
        if !in_single_quote && !in_double_quote && !in_backtick && !in_line_comment && !in_block_comment {
            if ch == '-' && next == '-' {
                in_line_comment = true;
                i += 2;
                continue;
            }
            if ch == '/' && next == '*' {
                in_block_comment = true;
                i += 2;
                continue;
            }
        }

        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            current.push(ch);
            i += 1;
            continue;
        }

        if in_block_comment {
            if ch == '*' && next == '/' {
                in_block_comment = false;
                current.push(ch);
                current.push(next);
                i += 2;
            } else {
                current.push(ch);
                i += 1;
            }
            continue;
        }

        if in_single_quote {
            if ch == '\\' {
                current.push(ch);
                current.push(next);
                i += 2;
            } else if ch == '\'' {
                in_single_quote = false;
                current.push(ch);
                i += 1;
            } else {
                current.push(ch);
                i += 1;
            }
            continue;
        }

        if in_double_quote {
            if ch == '\\' {
                current.push(ch);
                current.push(next);
                i += 2;
            } else if ch == '"' {
                in_double_quote = false;
                current.push(ch);
                i += 1;
            } else {
                current.push(ch);
                i += 1;
            }
            continue;
        }

        if in_backtick {
            if ch == '`' {
                in_backtick = false;
                current.push(ch);
                i += 1;
            } else {
                current.push(ch);
                i += 1;
            }
            continue;
        }

        // 普通模式
        if ch == '\'' {
            in_single_quote = true;
            current.push(ch);
            i += 1;
        } else if ch == '"' {
            in_double_quote = true;
            current.push(ch);
            i += 1;
        } else if ch == '`' {
            in_backtick = true;
            current.push(ch);
            i += 1;
        } else if ch == ';' {
            let stmt = current.trim().to_string();
            if !stmt.is_empty() {
                statements.push(stmt);
            }
            current.clear();
            i += 1;
        } else {
            current.push(ch);
            i += 1;
        }
    }
    let stmt = current.trim().to_string();
    if !stmt.is_empty() {
        statements.push(stmt);
    }
    statements
}

fn simple_format_sql(sql: &str) -> String {
    // 先尝试 DDL 格式化
    if let Some(ddl) = format_create_table_ddl(sql) {
        return ddl;
    }

    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // 统一大小写，方便关键词匹配
    let _upper = trimmed.to_ascii_uppercase();

    // 主关键词（独立一行，大写）
    let major_keywords = [
        "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING",
        "LIMIT", "INSERT INTO", "VALUES", "UPDATE", "SET", "DELETE FROM",
        "CREATE TABLE", "ALTER TABLE", "DROP TABLE", "CREATE INDEX",
        "CREATE VIEW", "CREATE DATABASE", "USE",
    ];

    // 连接/子句关键词（缩进一行）
    let clause_keywords = [
        "LEFT JOIN", "RIGHT JOIN", "INNER JOIN", "OUTER JOIN",
        "CROSS JOIN", "NATURAL JOIN", "JOIN", "ON", "AND", "OR",
        "UNION", "UNION ALL", "INTERSECT", "EXCEPT",
    ];

    // 第一步：分词，保留字符串和标识符
    #[derive(Debug, Clone)]
    enum Token {
        Word(String),
        Whitespace,
        Comma,
        OpenParen,
        CloseParen,
        Semicolon,
    }

    let mut tokens: Vec<Token> = Vec::new();
    let chars: Vec<char> = trimmed.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_whitespace() {
            tokens.push(Token::Whitespace);
            i += 1;
        } else if ch == ',' {
            tokens.push(Token::Comma);
            i += 1;
        } else if ch == '(' {
            tokens.push(Token::OpenParen);
            i += 1;
        } else if ch == ')' {
            tokens.push(Token::CloseParen);
            i += 1;
        } else if ch == ';' {
            tokens.push(Token::Semicolon);
            i += 1;
        } else if ch == '\'' || ch == '"' || ch == '`' {
            let quote = ch;
            let mut s = String::new();
            s.push(ch);
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    s.push(chars[i]);
                    i += 1;
                    if i < chars.len() {
                        s.push(chars[i]);
                        i += 1;
                    }
                } else if chars[i] == quote {
                    s.push(chars[i]);
                    i += 1;
                    break;
                } else {
                    s.push(chars[i]);
                    i += 1;
                }
            }
            tokens.push(Token::Word(s));
        } else {
            let mut s = String::new();
            while i < chars.len()
                && !chars[i].is_whitespace()
                && chars[i] != ','
                && chars[i] != '('
                && chars[i] != ')'
                && chars[i] != ';'
                && chars[i] != '\''
                && chars[i] != '"'
                && chars[i] != '`'
            {
                s.push(chars[i]);
                i += 1;
            }
            tokens.push(Token::Word(s));
        }
    }

    // 第二步：按主关键词分行
    let mut lines: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut i = 0;
    let mut indent = false;

    let is_keyword_match = |tokens: &[Token], start: usize, kw: &str| -> bool {
        let kw_tokens: Vec<&str> = kw.split_whitespace().collect();
        let mut ti = start;
        for (ki, kw_part) in kw_tokens.iter().enumerate() {
            // 跳过空格
            while ti < tokens.len() && matches!(&tokens[ti], Token::Whitespace) {
                ti += 1;
            }
            if ti >= tokens.len() {
                return false;
            }
            match &tokens[ti] {
                Token::Word(w) if w.to_ascii_uppercase() == *kw_part => {
                    ti += 1;
                    // 单 token 关键字检查下一个是否为空格或标点
                    if ki == kw_tokens.len() - 1 {
                        if ti < tokens.len()
                            && !matches!(
                                &tokens[ti],
                                Token::Whitespace | Token::Comma | Token::Semicolon | Token::OpenParen | Token::CloseParen
                            )
                        {
                            return false;
                        }
                    }
                }
                _ => return false,
            }
        }
        true
    };

    while i < tokens.len() {
        // 跳过前导空格
        while i < tokens.len() && matches!(&tokens[i], Token::Whitespace) {
            i += 1;
        }
        if i >= tokens.len() {
            break;
        }

        // 检查是否为主关键词
        let mut matched_kw: Option<&str> = None;
        for kw in &major_keywords {
            if is_keyword_match(&tokens, i, kw) {
                matched_kw = Some(kw);
                break;
            }
        }

        if let Some(kw) = matched_kw {
            // 输出当前累积的文本
            if !current.is_empty() {
                let line: String = current.iter().cloned().collect();
                let line = line.trim().to_string();
                if !line.is_empty() {
                    if indent {
                        lines.push(format!("  {line}"));
                    } else {
                        lines.push(line);
                    }
                }
                current.clear();
            }
            indent = kw != "SELECT" && kw != "INSERT INTO" && kw != "UPDATE" && kw != "DELETE FROM"
                && kw != "CREATE TABLE" && kw != "ALTER TABLE" && kw != "CREATE VIEW"
                && kw != "CREATE DATABASE";
            // 吞掉关键词 token
            let kw_parts: Vec<&str> = kw.split_whitespace().collect();
            for _ in 0..kw_parts.len() {
                while i < tokens.len() && matches!(&tokens[i], Token::Whitespace) {
                    i += 1;
                }
                if i < tokens.len() {
                    let _ = &tokens[i];
                    i += 1;
                }
            }
            // 在关键词后换行
            current.push(kw.to_string());
            let line = current.join(" ");
            if !line.trim().is_empty() {
                lines.push(line.trim().to_string());
            }
            current.clear();
            // 下一行以逗号开头则需要缩进
            let mut peek = i;
            while peek < tokens.len() && matches!(&tokens[peek], Token::Whitespace) {
                peek += 1;
            }
            current.clear();
        } else {
            // 检查是否为从句关键词
            let mut clause_matched: Option<&str> = None;
            for ck in &clause_keywords {
                if is_keyword_match(&tokens, i, ck) {
                    clause_matched = Some(ck);
                    break;
                }
            }

            if let Some(ck) = clause_matched {
                // 输出当前累积文本
                if !current.is_empty() {
                    let line = current.join(" ");
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        lines.push(format!("  {line}"));
                    }
                    current.clear();
                }
                // 吞掉从句关键词
                let ck_parts: Vec<&str> = ck.split_whitespace().collect();
                for _ in 0..ck_parts.len() {
                    while i < tokens.len() && matches!(&tokens[i], Token::Whitespace) {
                        i += 1;
                    }
                    if i < tokens.len() {
                        i += 1;
                    }
                }
                lines.push(format!("  {ck}"));
            } else {
                // 普通 token，累积
                match &tokens[i] {
                    Token::Whitespace => {
                        current.push(" ".to_string());
                        i += 1;
                    }
                    Token::Comma => {
                        current.push(",".to_string());
                        i += 1;
                    }
                    Token::OpenParen => {
                        current.push("(".to_string());
                        i += 1;
                    }
                    Token::CloseParen => {
                        current.push(")".to_string());
                        i += 1;
                    }
                    Token::Semicolon => {
                        // 分号：输出当前行然后单独输出分号
                        if !current.is_empty() {
                            let line = current.join(" ");
                            let line = line.trim().to_string();
                            if !line.is_empty() {
                                lines.push(line);
                            }
                            current.clear();
                        }
                        lines.push(";".to_string());
                        i += 1;
                    }
                    Token::Word(w) => {
                        current.push(w.clone());
                        i += 1;
                    }
                }
            }
        }
    }

    // 输出剩余内容
    if !current.is_empty() {
        let line = current.join(" ");
        let line = line.trim().to_string();
        if !line.is_empty() {
            lines.push(line);
        }
    }

    // 后处理：合并过短的行、处理逗号
    let mut result: Vec<String> = Vec::new();
    for (_idx, line) in lines.iter().enumerate() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        // 如果当前行以逗号开头，附加到上一行
        if trimmed_line.starts_with(',') && !result.is_empty() {
            let last = result.last_mut().unwrap();
            last.push_str(trimmed_line);
            continue;
        }

        // 如果上一行以逗号结尾，与当前行合并
        if !result.is_empty() && result.last().unwrap().trim_end().ends_with(',') {
            let last = result.last_mut().unwrap();
            last.push(' ');
            last.push_str(trimmed_line);
            continue;
        }

        result.push(trimmed_line.to_string());
    }

    result.join("\n")
}

fn definition_editor_palette(visuals: &egui::Visuals) -> EditorPalette {
    editor_palette(visuals)
}

fn blend_color(left: Color32, right: Color32, right_weight: f32) -> Color32 {
    let clamped = right_weight.clamp(0.0, 1.0);
    let left_weight = 1.0 - clamped;
    let mix = |a: u8, b: u8| ((a as f32 * left_weight) + (b as f32 * clamped)).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(left.r(), right.r()),
        mix(left.g(), right.g()),
        mix(left.b(), right.b()),
        mix(left.a(), right.a()),
    )
}

fn toolbar_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: ToolbarButtonKind,
) -> egui::Response {
    let palette = mac_ui_palette(ui.visuals());
    let (fill, text, stroke) = match kind {
        ToolbarButtonKind::Primary => (
            palette.primary_button_bg,
            palette.primary_button_text,
            Stroke::new(1.0, palette.primary_button_stroke),
        ),
        ToolbarButtonKind::Secondary => (
            palette.secondary_button_bg,
            palette.secondary_button_text,
            Stroke::new(1.0, palette.secondary_button_stroke),
        ),
        ToolbarButtonKind::Accent => (
            palette.accent_button_bg,
            palette.accent_button_text,
            Stroke::new(1.0, palette.accent_button_stroke),
        ),
        ToolbarButtonKind::Subtle => (
            palette.subtle_button_bg,
            palette.subtle_button_text,
            Stroke::new(1.0, palette.subtle_button_stroke),
        ),
    };

    ui.add(
        egui::Button::new(RichText::new(label).size(12.5).color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(5.0)
            .min_size(Vec2::new(0.0, 26.0)),
    )
}

fn mini_button(ui: &mut egui::Ui, label: &str, kind: MiniButtonKind) -> egui::Response {
    let palette = mac_ui_palette(ui.visuals());
    let (fill, text, stroke) = match kind {
        MiniButtonKind::Subtle => (
            palette.subtle_button_bg,
            palette.subtle_button_text,
            Stroke::new(1.0, palette.subtle_button_stroke),
        ),
        MiniButtonKind::Danger => (
            palette.danger_button_bg,
            palette.danger_button_text,
            Stroke::new(1.0, palette.danger_button_stroke),
        ),
        MiniButtonKind::Accent => (
            palette.accent_button_bg,
            palette.accent_button_text,
            Stroke::new(1.0, palette.accent_button_stroke),
        ),
    };

    ui.add(
        egui::Button::new(RichText::new(label).size(11.5).color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(4.0)
            .min_size(Vec2::new(34.0, 22.0)),
    )
}

struct TabButtonOutput {
    tab_response: egui::Response,
    tab_clicked: bool,
    close_clicked: bool,
}

fn tab_button(
    ui: &mut egui::Ui,
    index: usize,
    icon: &str,
    label: &str,
    selected: bool,
) -> TabButtonOutput {
    let palette = mac_ui_palette(ui.visuals());
    let display_label = truncate_ui_label(label, 16);
    let is_truncated = display_label != label;
    let desired_width = (display_label.chars().count() as f32 * 7.0 + 62.0).clamp(96.0, 170.0);
    let desired_size = Vec2::new(desired_width, if selected { 30.0 } else { 28.0 });
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    response
        .clone()
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        Vec2::new(16.0, 16.0),
    );
    let close_response = ui.interact(
        close_rect,
        ui.make_persistent_id(("workspace-tab-close", index)),
        egui::Sense::click(),
    );
    close_response
        .clone()
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let fill = if selected {
            palette.workspace_bg
        } else if response.hovered() {
            palette.search_bg
        } else {
            palette.tab_idle_bg
        };
        let stroke = if selected {
            Stroke::new(1.0, palette.selection_stroke)
        } else {
            Stroke::new(1.0, palette.soft_border)
        };

        ui.painter().rect(
            rect,
            6.0,
            fill,
            stroke,
            egui::StrokeKind::Outside,
        );

        if selected {
            ui.painter().line_segment(
                [
                    egui::pos2(rect.left() + 8.0, rect.top() + 1.0),
                    egui::pos2(rect.right() - 8.0, rect.top() + 1.0),
                ],
                Stroke::new(2.0, palette.selection_stroke),
            );
            ui.painter().line_segment(
                [
                    egui::pos2(rect.left() + 1.0, rect.bottom() - 1.0),
                    egui::pos2(rect.right() - 1.0, rect.bottom() - 1.0),
                ],
                Stroke::new(2.0, palette.workspace_bg),
            );
        }

        ui.painter().text(
            egui::pos2(rect.left() + 9.0, rect.center().y),
            Align2::LEFT_CENTER,
            icon,
            FontId::new(12.5, FontFamily::Proportional),
            if selected {
                palette.selection_text
            } else {
                palette.weak_text
            },
        );

        ui.painter().text(
            egui::pos2(rect.left() + 24.0, rect.center().y),
            Align2::LEFT_CENTER,
            display_label,
            FontId::new(12.5, FontFamily::Proportional),
            if selected {
                palette.selection_text
            } else {
                palette.text
            },
        );

        let close_fill = if close_response.hovered() {
            palette.selection_bg
        } else {
            Color32::TRANSPARENT
        };
        if close_fill != Color32::TRANSPARENT {
            ui.painter().circle_filled(close_rect.center(), 7.0, close_fill);
        }
        ui.painter().text(
            close_rect.center(),
            Align2::CENTER_CENTER,
            "×",
            FontId::new(12.0, FontFamily::Proportional),
            if close_response.hovered() {
                palette.selection_text
            } else if selected {
                palette.weak_text
            } else {
                palette.weak_text
            },
        );
    }

    TabButtonOutput {
        tab_response: if is_truncated {
            response
                .clone()
                .on_hover_text(label.to_string())
        } else {
            response.clone()
        },
        tab_clicked: response.clicked(),
        close_clicked: close_response.clicked(),
    }
}

fn segment_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let palette = mac_ui_palette(ui.visuals());
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .size(12.0)
                .color(if selected {
                    palette.selection_text
                } else {
                    palette.weak_text
                }),
        )
        .fill(if selected {
            palette.selection_bg
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                palette.selection_stroke
            } else {
                palette.soft_border
            },
        ))
        .corner_radius(5.0)
        .min_size(Vec2::new(0.0, 24.0)),
    )
}

fn render_query_empty_state(ui: &mut egui::Ui, title: &str, description: &str) {
    let palette = mac_ui_palette(ui.visuals());
    let width = ui.available_width().max(220.0);
    let height = ui.available_height().max(140.0);
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
        |ui| {
            egui::Frame::new()
                .fill(palette.search_bg)
                .stroke(Stroke::new(1.0, palette.soft_border))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(18, 16))
                .show(ui, |ui| {
                    ui.set_max_width(320.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(title).strong().color(palette.text));
                        ui.add_space(4.0);
                        ui.small(RichText::new(description).color(palette.weak_text));
                    });
                });
        },
    );
}

fn compact_query_preview(sql: &str) -> String {
    let first_line = sql
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("空查询");
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "空查询".into()
    } else {
        compact
    }
}

fn load_query_library(
    services: &AppServices,
    connection_id: &str,
) -> (Vec<String>, Vec<SavedQueryEntry>) {
    (
        services.list_query_history(connection_id).unwrap_or_default(),
        services.list_saved_queries(connection_id).unwrap_or_default(),
    )
}

fn tab_icon_symbol(tab: &impl TabKindMarker) -> &'static str {
    tab.tab_icon()
}

trait TabKindMarker {
    fn tab_icon(&self) -> &'static str;
}

impl TabKindMarker for QueryTabState {
    fn tab_icon(&self) -> &'static str {
        "⌘"
    }
}

impl TabKindMarker for TableTabState {
    fn tab_icon(&self) -> &'static str {
        "▦"
    }
}

fn truncate_ui_label(label: &str, max_chars: usize) -> String {
    let total = label.chars().count();
    if total <= max_chars {
        return label.to_string();
    }

    let mut truncated = label.chars().take(max_chars.saturating_sub(1)).collect::<String>();
    truncated.push('…');
    truncated
}

/// Truncate label by display width (CJK = 2, ASCII = 1) so it fits a pixel budget.
fn truncate_ui_label_by_width(label: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthChar;

    let total_width: usize = label.chars().map(|c| c.width().unwrap_or(1)).sum();
    if total_width <= max_width {
        return label.to_string();
    }

    let mut width = 0usize;
    let mut chars = label.chars();
    let mut truncated = String::new();
    // Leave room for the ellipsis (width 1)
    let limit = max_width.saturating_sub(1);
    while width < limit {
        let Some(c) = chars.next() else { break };
        let cw = c.width().unwrap_or(1);
        if width + cw > limit {
            break;
        }
        width += cw;
        truncated.push(c);
    }
    truncated.push('…');
    truncated
}

fn connection_kind_badge(kind: &DatabaseKind) -> RichText {
    let label = match kind {
        DatabaseKind::MySql => "MySQL",
        DatabaseKind::Postgres => "PostgreSQL",
    };
    RichText::new(label).size(10.5)
}

fn node_icon_symbol(node_type: ExplorerNodeType) -> &'static str {
    match node_type {
        ExplorerNodeType::Connection => "◎",
        ExplorerNodeType::Database => "◫",
        ExplorerNodeType::Schema => "◇",
        ExplorerNodeType::Table => "▦",
        ExplorerNodeType::View => "◪",
    }
}

fn tree_row_button(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    strong: bool,
    width: f32,
) -> egui::Response {
    let palette = mac_ui_palette(ui.visuals());
    let desired_size = Vec2::new(width.max(24.0).min(ui.available_width()), 20.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    response
        .clone()
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let fill = if selected {
            palette.selection_bg
        } else {
            Color32::TRANSPARENT
        };
        let stroke = if selected {
            Stroke::new(1.0, palette.selection_stroke)
        } else {
            Stroke::NONE
        };

        if fill != Color32::TRANSPARENT || stroke != Stroke::NONE {
            ui.painter().rect(
                rect,
                4.0,
                fill,
                stroke,
                egui::StrokeKind::Outside,
            );
        }

        ui.painter().text(
            egui::pos2(rect.left() + 6.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::new(if strong { 13.0 } else { 12.5 }, FontFamily::Proportional),
            if selected {
                palette.selection_text
            } else {
                palette.text
            },
        );
    }

    response
}

fn mac_sidebar_palette_dark() -> MacUiPalette {
    MacUiPalette {
        sidebar_bg: Color32::from_rgb(40, 43, 48),
        ..mac_ui_palette(&egui::Visuals::dark())
    }
}

fn mac_sidebar_palette_light() -> MacUiPalette {
    MacUiPalette {
        sidebar_bg: Color32::from_rgb(230, 232, 235),
        ..mac_ui_palette(&egui::Visuals::light())
    }
}

fn mac_ui_palette(visuals: &egui::Visuals) -> MacUiPalette {
    if visuals.dark_mode {
        MacUiPalette {
            toolbar_bg: Color32::from_rgb(48, 51, 57),
            sidebar_bg: Color32::from_rgb(45, 48, 54),
            workspace_bg: Color32::from_rgb(53, 56, 62),
            card_bg: Color32::from_rgb(52, 56, 62),
            table_header_bg: Color32::from_rgb(61, 65, 72),
            table_alt_bg: Color32::from_rgb(49, 52, 58),
            search_bg: Color32::from_rgb(58, 62, 68),
            border: Color32::from_rgb(80, 85, 94),
            soft_border: Color32::from_rgb(91, 96, 106),
            table_grid: Color32::from_rgb(95, 100, 109),
            selection_bg: Color32::from_rgb(88, 135, 200),
            selection_stroke: Color32::from_rgb(135, 170, 225),
            selection_text: Color32::from_rgb(243, 247, 252),
            text: Color32::from_rgb(236, 239, 244),
            weak_text: Color32::from_rgb(188, 194, 203),
            muted_dot: Color32::from_rgb(131, 137, 148),
            success: Color32::from_rgb(70, 191, 128),
            danger: Color32::from_rgb(255, 117, 117),
            warning: Color32::from_rgb(255, 191, 71),
            tab_idle_bg: Color32::from_rgb(54, 58, 64),
            primary_button_bg: Color32::from_rgb(10, 132, 255),
            primary_button_stroke: Color32::from_rgb(70, 158, 255),
            primary_button_text: Color32::WHITE,
            secondary_button_bg: Color32::from_rgb(72, 77, 85),
            secondary_button_stroke: Color32::from_rgb(98, 104, 115),
            secondary_button_text: Color32::from_rgb(240, 242, 246),
            accent_button_bg: Color32::from_rgb(46, 138, 94),
            accent_button_stroke: Color32::from_rgb(78, 170, 126),
            accent_button_text: Color32::WHITE,
            subtle_button_bg: Color32::from_rgb(54, 57, 63),
            subtle_button_stroke: Color32::from_rgb(76, 81, 90),
            subtle_button_text: Color32::from_rgb(206, 211, 220),
            danger_button_bg: Color32::from_rgb(92, 58, 58),
            danger_button_stroke: Color32::from_rgb(126, 74, 74),
            danger_button_text: Color32::from_rgb(255, 229, 229),
        }
    } else {
        MacUiPalette {
            toolbar_bg: Color32::from_rgb(249, 250, 252),
            sidebar_bg: Color32::from_rgb(238, 239, 241),
            workspace_bg: Color32::from_rgb(250, 250, 251),
            card_bg: Color32::from_rgb(255, 255, 255),
            table_header_bg: Color32::from_rgb(242, 244, 247),
            table_alt_bg: Color32::from_rgb(249, 250, 252),
            search_bg: Color32::from_rgb(252, 252, 253),
            border: Color32::from_rgb(220, 224, 230),
            soft_border: Color32::from_rgb(229, 232, 237),
            table_grid: Color32::from_rgb(228, 232, 238),
            selection_bg: Color32::from_rgb(205, 225, 252),
            selection_stroke: Color32::from_rgb(127, 167, 226),
            selection_text: Color32::from_rgb(22, 63, 126),
            text: Color32::from_rgb(44, 52, 64),
            weak_text: Color32::from_rgb(109, 118, 130),
            muted_dot: Color32::from_rgb(150, 156, 166),
            success: Color32::from_rgb(48, 167, 104),
            danger: Color32::from_rgb(220, 86, 86),
            warning: Color32::from_rgb(255, 179, 25),
            tab_idle_bg: Color32::from_rgb(244, 246, 249),
            primary_button_bg: Color32::from_rgb(0, 122, 255),
            primary_button_stroke: Color32::from_rgb(0, 115, 239),
            primary_button_text: Color32::WHITE,
            secondary_button_bg: Color32::from_rgb(242, 244, 247),
            secondary_button_stroke: Color32::from_rgb(219, 224, 231),
            secondary_button_text: Color32::from_rgb(58, 67, 79),
            accent_button_bg: Color32::from_rgb(216, 241, 227),
            accent_button_stroke: Color32::from_rgb(171, 219, 192),
            accent_button_text: Color32::from_rgb(34, 113, 70),
            subtle_button_bg: Color32::from_rgb(248, 249, 251),
            subtle_button_stroke: Color32::from_rgb(228, 232, 238),
            subtle_button_text: Color32::from_rgb(97, 106, 118),
            danger_button_bg: Color32::from_rgb(255, 240, 240),
            danger_button_stroke: Color32::from_rgb(242, 201, 201),
            danger_button_text: Color32::from_rgb(161, 54, 54),
        }
    }
}

fn sql_highlight_job(sql: &str, visuals: &egui::Visuals) -> egui::text::LayoutJob {
    sql_highlight_job_with_font_size(sql, visuals, 15.0)
}

fn sql_highlight_job_with_font_size(
    sql: &str,
    visuals: &egui::Visuals,
    font_size: f32,
) -> egui::text::LayoutJob {
    let palette = editor_palette(visuals);
    let mut job = egui::text::LayoutJob::default();
    let default = TextFormat {
        font_id: FontId::new(font_size, FontFamily::Monospace),
        color: palette.text,
        ..Default::default()
    };
    let keyword = TextFormat {
        font_id: FontId::new(font_size, FontFamily::Monospace),
        color: palette.keyword,
        ..Default::default()
    };
    let string = TextFormat {
        font_id: FontId::new(font_size, FontFamily::Monospace),
        color: palette.string,
        ..Default::default()
    };
    let number = TextFormat {
        font_id: FontId::new(font_size, FontFamily::Monospace),
        color: palette.number,
        ..Default::default()
    };
    let comment = TextFormat {
        font_id: FontId::new(font_size, FontFamily::Monospace),
        color: palette.comment,
        ..Default::default()
    };

    let keywords = [
        "SELECT", "FROM", "WHERE", "ORDER", "BY", "GROUP", "HAVING", "LIMIT", "INSERT", "INTO",
        "VALUES", "UPDATE", "SET", "DELETE", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON",
        "AS", "AND", "OR", "NOT", "NULL", "IS", "IN", "EXISTS", "CREATE", "ALTER", "DROP",
        "TABLE", "VIEW", "DATABASE", "SCHEMA", "INDEX", "PRIMARY", "KEY", "DISTINCT", "UNION",
        "ALL", "CASE", "WHEN", "THEN", "ELSE", "END", "LIKE", "DESC", "ASC", "OFFSET",
    ];

    let mut i = 0;
    let bytes = sql.as_bytes();
    while i < bytes.len() {
        let ch = sql[i..].chars().next().unwrap_or_default();
        let ch_len = ch.len_utf8();

        if ch == '-' && sql[i..].starts_with("--") {
            let end = sql[i..]
                .find('\n')
                .map(|offset| i + offset)
                .unwrap_or(sql.len());
            job.append(&sql[i..end], 0.0, comment.clone());
            i = end;
            continue;
        }

        if ch == '/' && sql[i..].starts_with("/*") {
            let end = sql[i + 2..]
                .find("*/")
                .map(|offset| i + 4 + offset)
                .unwrap_or(sql.len());
            job.append(&sql[i..end], 0.0, comment.clone());
            i = end;
            continue;
        }

        if ch == '\'' || ch == '"' || ch == '`' {
            let quote = ch;
            let mut end = i + ch_len;
            while end < sql.len() {
                let next = sql[end..].chars().next().unwrap_or_default();
                end += next.len_utf8();
                if next == quote {
                    break;
                }
            }
            job.append(&sql[i..end], 0.0, string.clone());
            i = end;
            continue;
        }

        if ch.is_ascii_digit() {
            let mut end = i + ch_len;
            while end < sql.len() {
                let next = sql[end..].chars().next().unwrap_or_default();
                if !(next.is_ascii_digit() || next == '.') {
                    break;
                }
                end += next.len_utf8();
            }
            job.append(&sql[i..end], 0.0, number.clone());
            i = end;
            continue;
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut end = i + ch_len;
            while end < sql.len() {
                let next = sql[end..].chars().next().unwrap_or_default();
                if !(next.is_ascii_alphanumeric() || next == '_') {
                    break;
                }
                end += next.len_utf8();
            }
            let token = &sql[i..end];
            let upper = token.to_ascii_uppercase();
            if keywords.contains(&upper.as_str()) {
                job.append(token, 0.0, keyword.clone());
            } else {
                job.append(token, 0.0, default.clone());
            }
            i = end;
            continue;
        }

        job.append(&sql[i..i + ch_len], 0.0, default.clone());
        i += ch_len;
    }

    job
}

#[derive(Clone, Copy)]
struct EditorPalette {
    panel_bg: Color32,
    editor_bg: Color32,
    gutter_bg: Color32,
    current_line_bg: Color32,
    text: Color32,
    line_number: Color32,
    line_number_active: Color32,
    keyword: Color32,
    string: Color32,
    number: Color32,
    comment: Color32,
}

fn editor_palette(visuals: &egui::Visuals) -> EditorPalette {
    if visuals.dark_mode {
        EditorPalette {
            panel_bg: Color32::from_rgb(31, 37, 46),
            editor_bg: Color32::from_rgb(40, 43, 48),
            gutter_bg: Color32::from_rgb(36, 40, 46),
            current_line_bg: Color32::from_rgb(38, 53, 75),
            text: Color32::from_rgb(214, 222, 235),
            line_number: Color32::from_rgb(108, 121, 145),
            line_number_active: Color32::from_rgb(220, 228, 240),
            keyword: Color32::from_rgb(86, 156, 214),
            string: Color32::from_rgb(206, 145, 120),
            number: Color32::from_rgb(181, 206, 168),
            comment: Color32::from_rgb(106, 153, 85),
        }
    } else {
        EditorPalette {
            panel_bg: Color32::from_rgb(238, 241, 246),
            editor_bg: Color32::from_rgb(230, 232, 235),
            gutter_bg: Color32::from_rgb(224, 228, 234),
            current_line_bg: Color32::from_rgb(218, 231, 248),
            text: Color32::from_rgb(34, 42, 56),
            line_number: Color32::from_rgb(120, 132, 148),
            line_number_active: Color32::from_rgb(44, 72, 116),
            keyword: Color32::from_rgb(0, 92, 197),
            string: Color32::from_rgb(166, 88, 49),
            number: Color32::from_rgb(56, 130, 84),
            comment: Color32::from_rgb(105, 120, 105),
        }
    }
}

/// 提取编辑器渲染为独立函数，支持带/不带左侧面板的两种布局
fn render_query_editor(
    ui: &mut egui::Ui,
    tab: &mut QueryTabState,
    palette: &EditorPalette,
    editor_inner_height: f32,
    action: &mut TabUiAction,
) {
    egui::ScrollArea::vertical()
        .id_salt(format!("query-editor-scroll-{}", tab.id))
        .max_height(editor_inner_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let font_id = FontId::new(15.0, FontFamily::Monospace);
            let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font_id));
            let line_count = tab.sql.lines().count().max(1);
            let current_line =
                current_line_number(&tab.sql, tab.cursor_range);
            let mut layouter = |ui: &egui::Ui,
                                buf: &dyn egui::TextBuffer,
                                wrap_width: f32| {
                let mut job = sql_highlight_job(buf.as_str(), ui.visuals());
                job.wrap.max_width = wrap_width;
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            };

            ui.set_min_height(editor_inner_height);
            StripBuilder::new(ui)
                .size(Size::exact(42.0))
                .size(Size::remainder())
                .horizontal(|mut strip| {
                    strip.cell(|ui| {
                        let rect = ui.max_rect();
                        let painter = ui.painter();
                        painter.rect_filled(rect, 0.0, palette.gutter_bg);

                        let text_x = rect.right() - 6.0;
                        // 10px = 匹配右侧 TextEdit inner_margin 的垂直 padding
                        let gutter_top_padding = 10.0;
                        let mut y = rect.top() + gutter_top_padding + row_height * 0.5;
                        for row in 0..line_count {
                            let line = row + 1;
                            let is_current = current_line == line;
                            if is_current {
                                let highlight_rect = egui::Rect::from_min_max(
                                    egui::pos2(rect.left() + 2.0, y - row_height * 0.5),
                                    egui::pos2(rect.right() - 2.0, y + row_height * 0.5),
                                );
                                painter.rect_filled(
                                    highlight_rect,
                                    4.0,
                                    palette.current_line_bg,
                                );
                            }
                            painter.text(
                                egui::pos2(text_x, y),
                                Align2::RIGHT_CENTER,
                                line.to_string(),
                                FontId::new(14.0, FontFamily::Monospace),
                                if is_current {
                                    palette.line_number_active
                                } else {
                                    palette.line_number
                                },
                            );
                            y += row_height;
                        }
                        ui.allocate_rect(rect, egui::Sense::hover());
                    });

                    strip.cell(|ui| {
                        egui::Frame::new()
                            .fill(palette.editor_bg)
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                let editor_id = egui::Id::from(format!("query-editor-{}", tab.id));
                                let te = TextEdit::multiline(&mut tab.sql)
                                    .id(editor_id)
                                    .code_editor()
                                    .font(FontId::new(15.0, FontFamily::Monospace))
                                    .text_color(palette.text)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(12)
                                    .frame(false)
                                    .layouter(&mut layouter)
                                    .hint_text("");
                                let output = te.show(ui);
                                tab.cursor_range = output.cursor_range;

                                // 右键菜单：执行选中SQL
                                output.response.context_menu(|ui| {
                                    let has_selection = tab.cursor_range
                                        .is_some_and(|r| !r.is_empty());
                                    if ui.add_enabled(
                                        has_selection,
                                        egui::Button::new("▶ 执行选中SQL"),
                                    ).clicked() {
                                        let selected = tab.cursor_range
                                            .and_then(|r| if !r.is_empty() { Some(r.slice_str(&tab.sql).to_string()) } else { None });
                                        *action = TabUiAction::ExecuteQuery(ExecuteMode::Selection(selected));
                                        ui.close();
                                    }
                                });
                                if tab.editor_focus_requested {
                                    output.response.request_focus();
                                    // 光标放到文本末尾
                                    let cursor_pos = egui::text::CCursor::new(tab.sql.len());
                                    if let Some(mut state) = TextEdit::load_state(ui.ctx(), editor_id) {
                                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(cursor_pos)));
                                        state.store(ui.ctx(), editor_id);
                                    }
                                    tab.editor_focus_requested = false;
                                }
                            });
                    });
                });
        });
}

/// 已保存查询折叠面板
fn render_saved_queries_panel(
    ui: &mut egui::Ui,
    tab: &mut QueryTabState,
    chrome: MacUiPalette,
    action: &mut TabUiAction,
) {
    let panel_palette = mac_ui_palette(ui.visuals());
    egui::Frame::new()
        .fill(panel_palette.card_bg)
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(8, 8))
        .show(ui, |ui| {
            // 标题栏
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("已保存查询")
                        .size(13.0)
                        .strong()
                        .color(chrome.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 折叠按钮
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("◀")
                                    .size(13.0)
                                    .color(chrome.weak_text),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .min_size(Vec2::new(22.0, 20.0)),
                        )
                        .clicked()
                    {
                        tab.saved_queries_panel_visible = false;
                    }
                });
            });
            ui.add_space(6.0);

            // 搜索过滤框
            ui.add(
                egui::TextEdit::singleline(&mut tab.saved_queries_filter)
                    .hint_text("搜索...")
                    .font(FontId::new(12.0, FontFamily::Proportional))
                    .desired_width(ui.available_width()),
            );
            ui.add_space(6.0);

            // 查询列表
            let filtered: Vec<&SavedQueryEntry> = if tab.saved_queries_filter.trim().is_empty() {
                tab.saved_queries.iter().collect()
            } else {
                let lower = tab.saved_queries_filter.to_lowercase();
                tab.saved_queries
                    .iter()
                    .filter(|e| {
                        e.title.to_lowercase().contains(&lower)
                            || e.sql_text.to_lowercase().contains(&lower)
                    })
                    .collect()
            };

            egui::ScrollArea::vertical()
                .id_salt(format!("saved-queries-list-{}", tab.id))
                .show(ui, |ui| {
                    if filtered.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(24.0);
                            ui.small(
                                RichText::new("没有匹配的保存查询")
                                    .color(chrome.weak_text),
                            );
                        });
                    } else {
                        let panel_width = ui.available_width();
                        let btn_width = panel_width - 24.0;
                        // monospace 12pt: each column is about 7 px wide
                        for entry in &filtered {
                            let full_title = &entry.title;
                            let max_cols = ((btn_width - 14.0) / 7.0) as usize;
                            let display_title = truncate_ui_label_by_width(full_title, max_cols.max(3));
                            let is_truncated = display_title.len() < full_title.len();

                            let is_selected = tab.selected_saved_query_id.as_deref() == Some(&entry.id);
                            let (fill, stroke_color, text_color) = if is_selected {
                                (
                                    panel_palette.accent_button_bg,
                                    panel_palette.accent_button_stroke,
                                    panel_palette.accent_button_text,
                                )
                            } else {
                                (chrome.search_bg, chrome.soft_border, chrome.text)
                            };

                            ui.horizontal(|ui| {
                                // 查询名称区域（左对齐）
                                let title_btn_width = btn_width - 24.0;
                                let (rect, item_response) = ui.allocate_exact_size(
                                    egui::vec2(title_btn_width, 22.0),
                                    egui::Sense::click(),
                                );
                                ui.painter().rect_filled(rect, 4.0, fill);
                                ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.0, stroke_color), egui::StrokeKind::Inside);
                                ui.painter().text(
                                    egui::pos2(rect.left() + 8.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &display_title,
                                    FontId::new(11.0, FontFamily::Monospace),
                                    text_color,
                                );
                                // 悬浮显示完整名称
                                if is_truncated {
                                    item_response.clone().on_hover_text(full_title.clone());
                                }

                                // 双击：加载到编辑器
                                if item_response.double_clicked() {
                                    tab.sql = entry.sql_text.clone();
                                    tab.connection_id = Some(entry.connection_id.clone());
                                    tab.database = entry.database.clone();
                                    tab.selected_saved_query_id = Some(entry.id.clone());
                                    tab.messages.push(format!("已加载保存查询：{}", entry.title));
                                }

                                // 右键菜单
                                item_response.context_menu(|ui| {
                                    if ui.button("重命名").clicked() {
                                        *action = TabUiAction::OpenRenameSavedQueryDialog((*entry).clone());
                                        ui.close();
                                    }
                                    if ui.button("删除").clicked() {
                                        *action = TabUiAction::PromptDeleteSavedQuery((*entry).clone());
                                        ui.close();
                                    }
                                });

                                // 删除按钮
                                let delete_response = ui.add_sized(
                                    [22.0, 22.0],
                                    egui::Button::new(
                                        RichText::new("✕")
                                            .size(9.0)
                                            .color(chrome.weak_text),
                                    )
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE),
                                );
                                if delete_response.clicked() {
                                    *action = TabUiAction::PromptDeleteSavedQuery((*entry).clone());
                                }
                            });
                            ui.add_space(2.0);
                        }
                    }
                });
        });
}
