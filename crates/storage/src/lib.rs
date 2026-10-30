//! SQLite storage for JenderalCode Desktop (PRD §11).
//!
//! A single bundled-SQLite connection guarded by a mutex. Desktop-app scale
//! makes this simple approach appropriate; every call is short-lived.

use anyhow::{Context, Result};
use jcode_common::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../migrations/001_init.sql")),
    ("002_session_policy", include_str!("../migrations/002_session_policy.sql")),
    ("003_indexes", include_str!("../migrations/003_indexes.sql")),
    ("004_raise_iteration_limit", include_str!("../migrations/004_raise_iteration_limit.sql")),
    ("005_show_hidden_files", include_str!("../migrations/005_show_hidden_files.sql")),
    ("006_message_reasoning", include_str!("../migrations/006_message_reasoning.sql")),
];

pub struct Db {
    conn: Mutex<Connection>,
}

// `Mutex<Connection>` is Send + Sync because `Connection: Send`, so no
// manual `unsafe impl` is needed.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Db>();
};

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    conn.pragma_update(None, "temp_store", "MEMORY").ok();
    Ok(())
}

/// Escape `%`, `_` and `\` for use in a `LIKE ... ESCAPE '\'` pattern.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Db {
    /// Open (creating if necessary) the database at `path` and run migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data dir {}", parent.display()))?;
        }
        let conn = Connection::open(path.as_ref())
            .with_context(|| format!("opening database {}", path.as_ref().display()))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        apply_pragmas(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    /// In-memory database, primarily for tests.
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               name TEXT NOT NULL UNIQUE,
               applied_at INTEGER NOT NULL
             );",
        )?;
        for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
            let applied: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM _migrations WHERE name = ?1",
                    params![name],
                    |r| r.get::<_, i64>(0),
                )
                .map(|c| c > 0)?;
            if !applied {
                conn.execute_batch(sql)
                    .with_context(|| format!("applying migration {name}"))?;
                conn.execute(
                    "INSERT INTO _migrations(name, applied_at) VALUES (?1, ?2)",
                    params![name, now_ms()],
                )?;
                tracing_migration_log(name, i);
            }
        }
        Ok(())
    }

    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    // ------------------------------------------------------------------
    // Workspaces
    // ------------------------------------------------------------------

    /// Insert or touch a workspace. Returns the up-to-date row.
    pub fn upsert_workspace(&self, root_path: &str, name: &str) -> Result<WorkspaceInfo> {
        self.with_conn(|conn| {
            let now = now_ms();
            conn.execute(
                "INSERT INTO workspaces(id, name, root_path, last_opened_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(root_path) DO UPDATE SET name = ?2, last_opened_at = ?4",
                params![new_id(), name, root_path, now],
            )?;
            let row = conn.query_row(
                "SELECT id, name, root_path, last_opened_at FROM workspaces WHERE root_path = ?1",
                params![root_path],
                |r| {
                    Ok(WorkspaceInfo {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        root_path: r.get(2)?,
                        last_opened_at: r.get(3)?,
                        is_git_repo: Path::new(&r.get::<_, String>(2)?).join(".git").exists(),
                    })
                },
            )?;
            Ok(row)
        })
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, root_path, last_opened_at FROM workspaces
                 ORDER BY last_opened_at DESC LIMIT 50",
            )?;
            let rows = stmt.query_map([], |r| {
                let root: String = r.get(2)?;
                Ok(WorkspaceInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    root_path: root.clone(),
                    last_opened_at: r.get(3)?,
                    is_git_repo: Path::new(&root).join(".git").exists(),
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn get_workspace_by_path(&self, root_path: &str) -> Result<Option<WorkspaceInfo>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, root_path, last_opened_at FROM workspaces WHERE root_path = ?1",
                params![root_path],
                |r| {
                    let root: String = r.get(2)?;
                    Ok(WorkspaceInfo {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        root_path: root.clone(),
                        last_opened_at: r.get(3)?,
                        is_git_repo: Path::new(&root).join(".git").exists(),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn get_workspace(&self, id: &str) -> Result<Option<WorkspaceInfo>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, root_path, last_opened_at FROM workspaces WHERE id = ?1",
                params![id],
                |r| {
                    let root: String = r.get(2)?;
                    Ok(WorkspaceInfo {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        root_path: root.clone(),
                        last_opened_at: r.get(3)?,
                        is_git_repo: Path::new(&root).join(".git").exists(),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn delete_workspace(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM workspaces WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Providers
    // ------------------------------------------------------------------

    pub fn save_provider(&self, p: &ProviderConfig) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO providers(id, name, kind, base_url, enabled, extra_json, credential_ref, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                   name = ?2, kind = ?3, base_url = ?4, enabled = ?5,
                   extra_json = ?6, credential_ref = ?7, updated_at = ?9",
                params![
                    p.id,
                    p.name,
                    p.kind.as_str(),
                    p.base_url,
                    p.enabled as i64,
                    serde_json::to_string(&p.extra)?,
                    p.credential_ref,
                    p.created_at,
                    p.updated_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_providers(&self) -> Result<Vec<ProviderConfig>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, kind, base_url, enabled, extra_json, credential_ref, created_at, updated_at
                 FROM providers ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], row_to_provider)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn get_provider(&self, id: &str) -> Result<Option<ProviderConfig>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, kind, base_url, enabled, extra_json, credential_ref, created_at, updated_at
                 FROM providers WHERE id = ?1",
                params![id],
                row_to_provider,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM models WHERE provider_id = ?1", params![id])?;
            conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Models
    // ------------------------------------------------------------------

    /// Replace the discovered (non-custom) models of a provider.
    pub fn replace_discovered_models(&self, provider_id: &str, models: &[ModelInfo]) -> Result<()> {
        self.with_conn(|conn| {
            let now = now_ms();
            for m in models {
                conn.execute(
                    "INSERT INTO models(id, provider_id, model_name, display_name, capabilities_json,
                                        input_price_per_mtok, output_price_per_mtok, custom, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)
                     ON CONFLICT(provider_id, model_name) DO UPDATE SET
                       display_name = ?4, capabilities_json = ?5, updated_at = ?8",
                    params![
                        m.id,
                        provider_id,
                        m.name,
                        m.display_name,
                        serde_json::to_string(&m.capabilities)?,
                        m.input_price_per_mtok,
                        m.output_price_per_mtok,
                        now,
                    ],
                )?;
            }
            // Drop discovered rows that no longer exist upstream.
            let names: Vec<String> = models.iter().map(|m| m.name.clone()).collect();
            if names.is_empty() {
                conn.execute(
                    "DELETE FROM models WHERE provider_id = ?1 AND custom = 0",
                    params![provider_id],
                )?;
            } else {
                let placeholders = names.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "DELETE FROM models WHERE provider_id = ? AND custom = 0 AND model_name NOT IN ({placeholders})"
                );
                let mut bind: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(provider_id.to_string())];
                for n in names {
                    bind.push(Box::new(n));
                }
                let refs: Vec<&dyn rusqlite::ToSql> = bind.iter().map(|b| b.as_ref()).collect();
                conn.execute(&sql, refs.as_slice())?;
            }
            Ok(())
        })
    }

    pub fn save_custom_model(&self, provider_id: &str, name: &str, display: Option<&str>, input_price: Option<f64>, output_price: Option<f64>) -> Result<ModelInfo> {
        self.with_conn(|conn| {
            let id = format!("{provider_id}/{name}");
            let display = display.unwrap_or(name).to_string();
            let now = now_ms();
            conn.execute(
                "INSERT INTO models(id, provider_id, model_name, display_name, capabilities_json, custom, updated_at)
                 VALUES (?1, ?2, ?3, ?4, '{}', 1, ?5)
                 ON CONFLICT(provider_id, model_name) DO UPDATE SET
                   display_name = ?4, custom = 1, input_price_per_mtok = ?6, output_price_per_mtok = ?7, updated_at = ?5",
                params![id, provider_id, name, display, now, input_price, output_price],
            )?;
            conn.execute(
                "UPDATE models SET input_price_per_mtok = ?1, output_price_per_mtok = ?2 WHERE id = ?3",
                params![input_price, output_price, id],
            )?;
            Ok(ModelInfo {
                id,
                provider_id: provider_id.into(),
                name: name.into(),
                display_name: display,
                capabilities: ModelCapabilities::default(),
                input_price_per_mtok: input_price,
                output_price_per_mtok: output_price,
                custom: true,
            })
        })
    }

    pub fn list_models(&self, provider_id: Option<&str>) -> Result<Vec<ModelInfo>> {
        self.with_conn(|conn| {
            let (sql, binds): (String, Vec<String>) = match provider_id {
                Some(pid) => (
                    "SELECT id, provider_id, model_name, display_name, capabilities_json,
                            input_price_per_mtok, output_price_per_mtok, custom
                     FROM models WHERE provider_id = ?1 ORDER BY display_name ASC"
                        .into(),
                    vec![pid.to_string()],
                ),
                None => (
                    "SELECT id, provider_id, model_name, display_name, capabilities_json,
                            input_price_per_mtok, output_price_per_mtok, custom
                     FROM models ORDER BY provider_id, display_name ASC"
                        .into(),
                    vec![],
                ),
            };
            let mut stmt = conn.prepare(&sql)?;
            let binds_ref: Vec<&dyn rusqlite::ToSql> =
                binds.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(binds_ref.as_slice(), |r| {
                let caps_json: String = r.get(4)?;
                Ok(ModelInfo {
                    id: r.get(0)?,
                    provider_id: r.get(1)?,
                    name: r.get(2)?,
                    display_name: r.get(3)?,
                    capabilities: serde_json::from_str(&caps_json).unwrap_or_default(),
                    input_price_per_mtok: r.get(5)?,
                    output_price_per_mtok: r.get(6)?,
                    custom: r.get::<_, i64>(7)? != 0,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn get_model(&self, id: &str) -> Result<Option<ModelInfo>> {
        self.list_models(None).map(|mut ms| {
            ms.retain(|m| m.id == id);
            ms.pop()
        })
    }

    pub fn delete_model(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM models WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Agent profiles
    // ------------------------------------------------------------------

    pub fn save_agent_profile(&self, p: &AgentProfile) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO agent_profiles(id, name, mode, system_prompt, allowed_tools_json, policy_mode, provider_id, model_id, built_in, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                   name = ?2, mode = ?3, system_prompt = ?4, allowed_tools_json = ?5,
                   policy_mode = ?6, provider_id = ?7, model_id = ?8",
                params![
                    p.id,
                    p.name,
                    p.mode.as_str(),
                    p.system_prompt,
                    p.allowed_tools.as_ref().map(|t| serde_json::to_string(t).unwrap_or_default()),
                    p.policy_mode.as_str(),
                    p.provider_id,
                    p.model_id,
                    p.built_in as i64,
                    p.created_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_agent_profiles(&self) -> Result<Vec<AgentProfile>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, mode, system_prompt, allowed_tools_json, policy_mode, provider_id, model_id, built_in, created_at
                 FROM agent_profiles ORDER BY built_in DESC, name ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                let mode: String = r.get(2)?;
                let tools_json: Option<String> = r.get(4)?;
                let policy: String = r.get(5)?;
                Ok(AgentProfile {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    mode: match mode.as_str() {
                        "plan" => AgentMode::Plan,
                        "build" => AgentMode::Build,
                        "review" => AgentMode::Review,
                        _ => AgentMode::Custom,
                    },
                    system_prompt: r.get(3)?,
                    allowed_tools: tools_json.and_then(|t| serde_json::from_str(&t).ok()),
                    policy_mode: PolicyMode::from_str_lossy(&policy),
                    provider_id: r.get(6)?,
                    model_id: r.get(7)?,
                    built_in: r.get::<_, i64>(8)? != 0,
                    created_at: r.get(9)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn delete_agent_profile(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM agent_profiles WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    // ------------------------------------------------------------------
    // Sessions & messages
    // ------------------------------------------------------------------

    pub fn create_session(
        &self,
        workspace_id: &str,
        title: &str,
        agent_profile_id: Option<&str>,
        provider_id: Option<&str>,
        model_name: Option<&str>,
    ) -> Result<SessionSummary> {
        self.with_conn(|conn| {
            let id = new_id();
            let now = now_ms();
            conn.execute(
                "INSERT INTO sessions(id, workspace_id, title, agent_profile_id, provider_id, model_name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![id, workspace_id, title, agent_profile_id, provider_id, model_name, now],
            )?;
            Ok(SessionSummary {
                id,
                workspace_id: workspace_id.into(),
                title: title.into(),
                agent_profile_id: agent_profile_id.map(String::from),
                provider_id: provider_id.map(String::from),
                model_name: model_name.map(String::from),
                policy_mode: None,
                created_at: now,
                updated_at: now,
                message_count: 0,
            })
        })
    }

    /// Persist a per-session policy override (survives restarts).
    pub fn set_session_policy_mode(&self, id: &str, mode: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET policy_mode = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, mode, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn update_session_model(&self, id: &str, provider_id: Option<&str>, model_name: Option<&str>) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET provider_id = ?2, model_name = ?3, updated_at = ?4 WHERE id = ?1",
                params![id, provider_id, model_name, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn update_session_agent(&self, id: &str, agent_profile_id: Option<&str>) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET agent_profile_id = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, agent_profile_id, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn rename_session(&self, id: &str, title: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, title, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn touch_session(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET updated_at = ?2 WHERE id = ?1",
                params![id, now_ms()],
            )?;
            Ok(())
        })
    }

    fn session_query(where_clause: &str) -> String {
        format!(
            "SELECT s.id, s.workspace_id, s.title, s.agent_profile_id, s.provider_id, s.model_name,
                    s.policy_mode, s.created_at, s.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.session_id = s.id)
             FROM sessions s {where_clause} ORDER BY s.updated_at DESC LIMIT 200"
        )
    }

    fn row_to_session(r: &rusqlite::Row) -> rusqlite::Result<SessionSummary> {
        let policy: Option<String> = r.get(6)?;
        Ok(SessionSummary {
            id: r.get(0)?,
            workspace_id: r.get(1)?,
            title: r.get(2)?,
            agent_profile_id: r.get(3)?,
            provider_id: r.get(4)?,
            model_name: r.get(5)?,
            policy_mode: policy.map(|p| PolicyMode::from_str_lossy(&p)),
            created_at: r.get(7)?,
            updated_at: r.get(8)?,
            message_count: r.get(9)?,
        })
    }

    pub fn list_sessions(&self, workspace_id: Option<&str>) -> Result<Vec<SessionSummary>> {
        self.with_conn(|conn| {
            match workspace_id {
                Some(wid) => {
                    let sql = Self::session_query("WHERE s.workspace_id = ?1");
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![wid], Self::row_to_session)?;
                    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
                }
                None => {
                    let sql = Self::session_query("");
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], Self::row_to_session)?;
                    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
                }
            }
        })
    }

    pub fn search_sessions(&self, query: &str) -> Result<Vec<SessionSummary>> {
        let like = format!("%{}%", escape_like(query));
        self.with_conn(|conn| {
            let sql = Self::session_query(
                "WHERE s.title LIKE ?1 ESCAPE '\\' OR s.id IN (SELECT session_id FROM messages WHERE content LIKE ?1 ESCAPE '\\')",
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params![like], Self::row_to_session)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<SessionSummary>> {
        self.with_conn(|conn| {
            let sql = Self::session_query("WHERE s.id = ?1");
            let mut stmt = conn.prepare(&sql)?;
            stmt.query_row(params![id], Self::row_to_session)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn append_message(&self, m: &MessageRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.prepare_cached(
                "INSERT INTO messages(id, session_id, role, content, tool_calls_json, tool_call_id, tool_name, created_at, reasoning)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?
            .execute(params![
                    m.id,
                    m.session_id,
                    m.role.as_str(),
                    m.content,
                    if m.tool_calls.is_empty() {
                        None
                    } else {
                        Some(serde_json::to_string(&m.tool_calls)?)
                    },
                    m.tool_call_id,
                    m.tool_name,
                    m.created_at,
                    m.reasoning,
                ])?;
            conn.prepare_cached("UPDATE sessions SET updated_at = ?2 WHERE id = ?1")?
                .execute(params![m.session_id, now_ms()])?;
            Ok(())
        })
    }

    /// Number of messages stored for a session.
    pub fn message_count(&self, session_id: &str) -> Result<i64> {
        self.with_conn(|conn| {
            conn.prepare_cached("SELECT COUNT(*) FROM messages WHERE session_id = ?1")?
                .query_row(params![session_id], |r| r.get(0))
                .map_err(Into::into)
        })
    }

    pub fn list_messages(&self, session_id: &str) -> Result<Vec<MessageRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, session_id, role, content, tool_calls_json, tool_call_id, tool_name, created_at, reasoning
                 FROM messages WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![session_id], |r| {
                let role: String = r.get(2)?;
                let tools_json: Option<String> = r.get(4)?;
                Ok(MessageRecord {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: match role.as_str() {
                        "system" => Role::System,
                        "user" => Role::User,
                        "assistant" => Role::Assistant,
                        _ => Role::Tool,
                    },
                    content: r.get(3)?,
                    tool_calls: tools_json.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
                    tool_call_id: r.get(5)?,
                    tool_name: r.get(6)?,
                    created_at: r.get(7)?,
                    reasoning: r.get(8)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    // ------------------------------------------------------------------
    // Tool runs & approvals
    // ------------------------------------------------------------------

    pub fn insert_tool_run(&self, t: &ToolRunRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.prepare_cached(
                "INSERT INTO tool_runs(id, message_id, session_id, tool_name, status, args_json, result_summary, error, approval_id, created_at, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![
                    t.id,
                    t.message_id,
                    t.session_id,
                    t.tool_name,
                    t.status.as_str(),
                    serde_json::to_string(&t.args)?,
                    t.result_summary,
                    t.error,
                    t.approval_id,
                    t.created_at,
                    t.finished_at,
                ])?;
            Ok(())
        })
    }

    pub fn update_tool_run(&self, id: &str, status: ToolRunStatus, result_summary: Option<&str>, error: Option<&str>) -> Result<()> {
        self.with_conn(|conn| {
            conn.prepare_cached(
                "UPDATE tool_runs SET status = ?2, result_summary = ?3, error = ?4, finished_at = ?5 WHERE id = ?1",
            )?
            .execute(params![id, status.as_str(), result_summary, error, now_ms()])?;
            Ok(())
        })
    }

    pub fn list_tool_runs(&self, session_id: &str) -> Result<Vec<ToolRunRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT id, message_id, session_id, tool_name, status, args_json, result_summary, error, approval_id, created_at, finished_at
                 FROM tool_runs WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![session_id], map_tool_run_row)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    pub fn get_tool_run(&self, id: &str) -> Result<Option<ToolRunRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, message_id, session_id, tool_name, status, args_json, result_summary, error, approval_id, created_at, finished_at
                 FROM tool_runs WHERE id = ?1",
            )?;
            stmt.query_row(params![id], map_tool_run_row)
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn insert_approval(&self, a: &ApprovalRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO approvals(id, tool_run_id, session_id, kind, description, preview, status, created_at, decided_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    a.id,
                    a.tool_run_id,
                    a.session_id,
                    a.kind,
                    a.description,
                    a.preview,
                    a.status.as_str(),
                    a.created_at,
                    a.decided_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn decide_approval(&self, id: &str, approved: bool) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE approvals SET status = ?2, decided_at = ?3 WHERE id = ?1",
                params![
                    id,
                    if approved { "approved" } else { "denied" },
                    now_ms()
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_approvals(&self, session_id: &str) -> Result<Vec<ApprovalRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, tool_run_id, session_id, kind, description, preview, status, created_at, decided_at
                 FROM approvals WHERE session_id = ?1 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![session_id], |r| {
                let status: String = r.get(6)?;
                Ok(ApprovalRecord {
                    id: r.get(0)?,
                    tool_run_id: r.get(1)?,
                    session_id: r.get(2)?,
                    kind: r.get(3)?,
                    description: r.get(4)?,
                    preview: r.get(5)?,
                    status: match status.as_str() {
                        "approved" => ApprovalStatus::Approved,
                        "denied" => ApprovalStatus::Denied,
                        _ => ApprovalStatus::Pending,
                    },
                    created_at: r.get(7)?,
                    decided_at: r.get(8)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
        })
    }

    // ------------------------------------------------------------------
    // Usage
    // ------------------------------------------------------------------

    pub fn insert_usage_record(&self, u: &UsageRecord) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO usage_records(id, session_id, provider_id, model_name, input_tokens, output_tokens, cost_estimate, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    u.id,
                    u.session_id,
                    u.provider_id,
                    u.model_name,
                    u.input_tokens as i64,
                    u.output_tokens as i64,
                    u.cost_estimate,
                    u.created_at,
                ],
            )?;
            Ok(())
        })
    }

    /// Token totals for one session plus the most recent request, whose
    /// input size approximates how full the model's context currently is.
    pub fn session_usage(&self, session_id: &str) -> Result<SessionUsage> {
        self.with_conn(|conn| {
            let (total_in, total_out, total_cost, requests): (i64, i64, Option<f64>, i64) = conn
                .prepare_cached(
                    "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                            SUM(cost_estimate), COUNT(*)
                     FROM usage_records WHERE session_id = ?1",
                )?
                .query_row(params![session_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
            let last: Option<(i64, i64)> = conn
                .prepare_cached(
                    "SELECT input_tokens, output_tokens FROM usage_records
                     WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                )?
                .query_row(params![session_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            Ok(SessionUsage {
                total_input: total_in as u64,
                total_output: total_out as u64,
                total_cost,
                requests: requests as u64,
                last_input: last.map_or(0, |l| l.0 as u64),
                last_output: last.map_or(0, |l| l.1 as u64),
            })
        })
    }

    pub fn usage_summary(&self) -> Result<UsageSummary> {
        self.with_conn(|conn| {
            let (total_in, total_out, total_cost, sessions): (i64, i64, Option<f64>, i64) =
                conn.query_row(
                    "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                            SUM(cost_estimate), COUNT(DISTINCT session_id)
                     FROM usage_records",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
            let mut by_provider = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT provider_id, SUM(input_tokens), SUM(output_tokens), SUM(cost_estimate)
                     FROM usage_records GROUP BY provider_id ORDER BY SUM(input_tokens)+SUM(output_tokens) DESC",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok(ProviderUsage {
                        provider_id: r.get(0)?,
                        input_tokens: r.get::<_, i64>(1)? as u64,
                        output_tokens: r.get::<_, i64>(2)? as u64,
                        cost_estimate: r.get(3)?,
                    })
                })?;
                for row in rows {
                    by_provider.push(row?);
                }
            }
            let mut by_day = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT date(created_at/1000, 'unixepoch') AS day,
                            SUM(input_tokens), SUM(output_tokens), SUM(cost_estimate)
                     FROM usage_records
                     WHERE created_at > ?1
                     GROUP BY day ORDER BY day ASC",
                )?;
                let cutoff = now_ms() - 30 * 24 * 3600 * 1000;
                let rows = stmt.query_map(params![cutoff], |r| {
                    Ok(DayUsage {
                        day: r.get(0)?,
                        input_tokens: r.get::<_, i64>(1)? as u64,
                        output_tokens: r.get::<_, i64>(2)? as u64,
                        cost_estimate: r.get(3)?,
                    })
                })?;
                for row in rows {
                    by_day.push(row?);
                }
            }
            Ok(UsageSummary {
                total_input: total_in as u64,
                total_output: total_out as u64,
                total_cost,
                sessions: sessions as u64,
                by_provider,
                by_day,
            })
        })
    }

    // ------------------------------------------------------------------
    // Settings, retention, export
    // ------------------------------------------------------------------

    pub fn get_settings(&self) -> Result<AppSettings> {
        self.with_conn(|conn| {
            let json: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = 'app'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            match json {
                Some(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
                None => Ok(AppSettings::default()),
            }
        })
    }

    pub fn save_settings(&self, s: &AppSettings) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO settings(key, value_json) VALUES ('app', ?1)
                 ON CONFLICT(key) DO UPDATE SET value_json = ?1",
                params![serde_json::to_string(s)?],
            )?;
            Ok(())
        })
    }

    /// Delete sessions not touched within `days`. Returns removed count.
    pub fn apply_retention(&self, days: u32) -> Result<usize> {
        let cutoff = now_ms() - days as i64 * 24 * 3600 * 1000;
        self.with_conn(|conn| {
            let n = conn.execute(
                "DELETE FROM sessions WHERE updated_at < ?1",
                params![cutoff],
            )?;
            Ok(n)
        })
    }

    /// Delete all sessions and their history. Returns removed count.
    pub fn clear_all_history(&self) -> Result<usize> {
        self.with_conn(|conn| {
            let n = conn.execute("DELETE FROM sessions", [])?;
            Ok(n)
        })
    }

    /// Export full history as a JSON document (for data portability).
    pub fn export_history_json(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Export {
            exported_at: i64,
            workspaces: Vec<WorkspaceInfo>,
            sessions: Vec<SessionSummary>,
            messages: Vec<MessageRecord>,
            tool_runs: Vec<ToolRecordSlim>,
            usage: Vec<UsageRecord>,
        }
        #[derive(Serialize)]
        struct ToolRecordSlim {
            id: String,
            session_id: String,
            tool_name: String,
            status: String,
            created_at: i64,
        }
        let workspaces = self.list_workspaces()?;
        let sessions = self.list_sessions(None)?;
        let mut messages = Vec::new();
        let mut tool_runs = Vec::new();
        let mut usage = Vec::new();
        for s in &sessions {
            messages.extend(self.list_messages(&s.id)?);
            for t in self.list_tool_runs(&s.id)? {
                tool_runs.push(ToolRecordSlim {
                    id: t.id,
                    session_id: t.session_id,
                    tool_name: t.tool_name,
                    status: t.status.as_str().into(),
                    created_at: t.created_at,
                });
            }
        }
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, session_id, provider_id, model_name, input_tokens, output_tokens, cost_estimate, created_at
                 FROM usage_records ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(UsageRecord {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    provider_id: r.get(2)?,
                    model_name: r.get(3)?,
                    input_tokens: r.get::<_, i64>(4)? as u64,
                    output_tokens: r.get::<_, i64>(5)? as u64,
                    cost_estimate: r.get(6)?,
                    created_at: r.get(7)?,
                })
            })?;
            for row in rows {
                usage.push(row?);
            }
            Ok(())
        })?;
        let doc = Export {
            exported_at: now_ms(),
            workspaces,
            sessions,
            messages,
            tool_runs,
            usage,
        };
        Ok(serde_json::to_string_pretty(&doc)?)
    }
}

fn map_tool_run_row(r: &rusqlite::Row) -> rusqlite::Result<ToolRunRecord> {
    let status: String = r.get(4)?;
    let args_json: String = r.get(5).unwrap_or_else(|_| "{}".into());
    Ok(ToolRunRecord {
        id: r.get(0)?,
        message_id: r.get(1)?,
        session_id: r.get(2)?,
        tool_name: r.get(3)?,
        status: parse_tool_run_status(&status),
        args: serde_json::from_str(&args_json).unwrap_or(serde_json::Value::Null),
        result_summary: r.get(6)?,
        error: r.get(7)?,
        approval_id: r.get(8)?,
        created_at: r.get(9)?,
        finished_at: r.get(10)?,
    })
}

fn row_to_provider(r: &rusqlite::Row) -> rusqlite::Result<ProviderConfig> {
    let kind: String = r.get(2)?;
    let extra: String = r.get(5).unwrap_or_else(|_| "{}".into());
    Ok(ProviderConfig {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: ProviderKind::parse(&kind).unwrap_or(ProviderKind::OpenAiCompatible),
        base_url: r.get(3)?,
        enabled: r.get::<_, i64>(4)? != 0,
        extra: serde_json::from_str(&extra).unwrap_or(serde_json::Value::Null),
        credential_ref: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

fn parse_tool_run_status(s: &str) -> ToolRunStatus {
    match s {
        "pending" => ToolRunStatus::Pending,
        "awaiting_approval" => ToolRunStatus::AwaitingApproval,
        "running" => ToolRunStatus::Running,
        "success" => ToolRunStatus::Success,
        "denied" => ToolRunStatus::Denied,
        "cancelled" => ToolRunStatus::Cancelled,
        _ => ToolRunStatus::Error,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionUsage {
    pub total_input: u64,
    pub total_output: u64,
    #[serde(default)]
    pub total_cost: Option<f64>,
    pub requests: u64,
    /// Input tokens of the latest model request (≈ current context size).
    pub last_input: u64,
    pub last_output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_input: u64,
    pub total_output: u64,
    #[serde(default)]
    pub total_cost: Option<f64>,
    pub sessions: u64,
    pub by_provider: Vec<ProviderUsage>,
    pub by_day: Vec<DayUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_estimate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayUsage {
    /// `YYYY-MM-DD`
    pub day: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_estimate: Option<f64>,
}

fn tracing_migration_log(name: &str, index: usize) {
    let _ = (name, index); // cheap hook; real logging happens in the app layer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db {
        Db::open_memory().expect("memory db")
    }

    #[test]
    fn migrations_are_idempotent() {
        let d = db();
        // Re-running open on the same data would re-check and skip.
        let count: i64 = d
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(count as usize, MIGRATIONS.len());
    }

    #[test]
    fn migrations_rerun_on_file_db_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        drop(Db::open(&path).unwrap());
        let d = Db::open(&path).unwrap();
        let count: i64 = d
            .with_conn(|c| {
                c.query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(count as usize, MIGRATIONS.len());
        let idx: i64 = d
            .with_conn(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_tool_runs_message'",
                    [],
                    |r| r.get(0),
                )
                .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn message_count_and_like_escaping() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p-like", "p").unwrap();
        let s = d.create_session(&w.id, "plain title", None, None, None).unwrap();
        assert_eq!(d.message_count(&s.id).unwrap(), 0);
        let m = MessageRecord {
            id: new_id(),
            session_id: s.id.clone(),
            role: Role::User,
            content: "100% done_ok".into(),
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            created_at: now_ms(),
        };
        d.append_message(&m).unwrap();
        assert_eq!(d.message_count(&s.id).unwrap(), 1);
        assert_eq!(d.search_sessions("100%").unwrap().len(), 1);
        assert_eq!(d.search_sessions("done_ok").unwrap().len(), 1);
        // `_` and `%` must be literal, not wildcards.
        assert!(d.search_sessions("done_o_").unwrap().is_empty());
        assert!(d.search_sessions("1%done").unwrap().is_empty());
    }

    #[test]
    fn message_reasoning_round_trips() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p-reason", "p").unwrap();
        let s = d.create_session(&w.id, "t", None, None, None).unwrap();
        for reasoning in [Some("thinking about it".to_string()), None] {
            d.append_message(&MessageRecord {
                id: new_id(),
                session_id: s.id.clone(),
                role: Role::Assistant,
                content: "answer".into(),
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: None,
                reasoning,
                created_at: now_ms(),
            })
            .unwrap();
        }
        let msgs = d.list_messages(&s.id).unwrap();
        assert_eq!(msgs[0].reasoning.as_deref(), Some("thinking about it"));
        assert_eq!(msgs[1].reasoning, None);
    }

    #[test]
    fn workspace_upsert_updates_last_opened() {
        let d = db();
        let w1 = d.upsert_workspace("/tmp/proj", "proj").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let w2 = d.upsert_workspace("/tmp/proj", "proj-renamed").unwrap();
        assert_eq!(w1.id, w2.id);
        assert_eq!(w2.name, "proj-renamed");
        assert!(w2.last_opened_at >= w1.last_opened_at);
        assert_eq!(d.list_workspaces().unwrap().len(), 1);
    }

    #[test]
    fn provider_and_models_roundtrip() {
        let d = db();
        let p = ProviderConfig {
            id: "prov1".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "https://api.openai.com/v1".into(),
            enabled: true,
            created_at: 1,
            updated_at: 1,
            extra: serde_json::json!({}),
            credential_ref: Some("jcode:prov1".into()),
        };
        d.save_provider(&p).unwrap();
        assert_eq!(d.list_providers().unwrap().len(), 1);

        let models = vec![ModelInfo {
            id: "prov1/gpt-4o".into(),
            provider_id: "prov1".into(),
            name: "gpt-4o".into(),
            display_name: "GPT-4o".into(),
            capabilities: ModelCapabilities {
                tool_calling: Some(true),
                streaming: Some(true),
                ..Default::default()
            },
            input_price_per_mtok: Some(2.5),
            output_price_per_mtok: Some(10.0),
            custom: false,
        }];
        d.replace_discovered_models("prov1", &models).unwrap();
        assert_eq!(d.list_models(Some("prov1")).unwrap().len(), 1);

        // A stale discovered model disappears after refresh.
        d.replace_discovered_models("prov1", &[]).unwrap();
        assert_eq!(d.list_models(Some("prov1")).unwrap().len(), 0);

        // Custom models survive refreshes.
        d.save_custom_model("prov1", "custom-model", None, None, None).unwrap();
        d.replace_discovered_models("prov1", &[]).unwrap();
        assert_eq!(d.list_models(Some("prov1")).unwrap().len(), 1);
    }

    #[test]
    fn session_message_history_roundtrip() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p2", "p2").unwrap();
        let s = d
            .create_session(&w.id, "My session", None, Some("prov1"), Some("gpt-4o"))
            .unwrap();
        let m = MessageRecord {
            id: new_id(),
            session_id: s.id.clone(),
            role: Role::User,
            content: "hello".into(),
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            created_at: now_ms(),
        };
        d.append_message(&m).unwrap();
        let msgs = d.list_messages(&s.id).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "hello");
        let sessions = d.list_sessions(Some(&w.id)).unwrap();
        assert_eq!(sessions[0].message_count, 1);
        assert!(d.search_sessions("hello").unwrap().len() == 1);

        d.delete_session(&s.id).unwrap();
        assert!(d.list_messages(&s.id).unwrap().is_empty());
    }

    #[test]
    fn session_policy_mode_persists() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p2b", "p2b").unwrap();
        let s = d.create_session(&w.id, "s", None, None, None).unwrap();
        assert_eq!(s.policy_mode, None);

        d.set_session_policy_mode(&s.id, "yolo").unwrap();
        let loaded = d.get_session(&s.id).unwrap().unwrap();
        assert_eq!(loaded.policy_mode, Some(PolicyMode::Yolo));

        // Also visible through list/search.
        assert_eq!(
            d.list_sessions(Some(&w.id)).unwrap()[0].policy_mode,
            Some(PolicyMode::Yolo)
        );

        // Switch back.
        d.set_session_policy_mode(&s.id, "safe").unwrap();
        assert_eq!(d.get_session(&s.id).unwrap().unwrap().policy_mode, Some(PolicyMode::Safe));
    }

    #[test]
    fn usage_summary_aggregates() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p3", "p3").unwrap();
        let s = d.create_session(&w.id, "s", None, Some("prov1"), Some("m1")).unwrap();
        for (i, o) in [(100u64, 50u64), (10, 20)] {
            d.insert_usage_record(&UsageRecord {
                id: new_id(),
                session_id: s.id.clone(),
                provider_id: "prov1".into(),
                model_name: "m1".into(),
                input_tokens: i,
                output_tokens: o,
                cost_estimate: Some(0.001),
                created_at: now_ms(),
            })
            .unwrap();
        }
        let sum = d.usage_summary().unwrap();
        assert_eq!(sum.total_input, 110);
        assert_eq!(sum.total_output, 70);
        assert!((sum.total_cost.unwrap() - 0.002).abs() < 1e-9);
        assert_eq!(sum.by_provider.len(), 1);
        assert_eq!(sum.by_day.len(), 1);
    }

    #[test]
    fn session_usage_totals_and_latest_request() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p4", "p4").unwrap();
        let s = d.create_session(&w.id, "s", None, Some("prov1"), Some("m1")).unwrap();
        let other = d.create_session(&w.id, "o", None, Some("prov1"), Some("m1")).unwrap();
        assert_eq!(d.session_usage(&s.id).unwrap(), SessionUsage::default());
        for (sid, i, o, at) in [(&s.id, 100u64, 50u64, 1_000i64), (&s.id, 900, 20, 2_000), (&other.id, 7, 7, 3_000)] {
            d.insert_usage_record(&UsageRecord {
                id: new_id(),
                session_id: sid.clone(),
                provider_id: "prov1".into(),
                model_name: "m1".into(),
                input_tokens: i,
                output_tokens: o,
                cost_estimate: None,
                created_at: at,
            })
            .unwrap();
        }
        let u = d.session_usage(&s.id).unwrap();
        assert_eq!((u.total_input, u.total_output, u.requests), (1000, 70, 2));
        assert_eq!((u.last_input, u.last_output), (900, 20));
        assert_eq!(u.total_cost, None);
    }

    #[test]
    fn hidden_files_migration_turns_the_old_default_on() {
        let d = db();
        let mut s = d.get_settings().unwrap();
        s.show_hidden_files = false;
        d.save_settings(&s).unwrap();
        d.with_conn(|c| {
            c.execute_batch(include_str!("../migrations/005_show_hidden_files.sql"))?;
            Ok(())
        })
        .unwrap();
        assert!(d.get_settings().unwrap().show_hidden_files);
    }

    #[test]
    fn old_default_iteration_limit_is_raised_by_migration() {
        let d = db();
        let mut s = d.get_settings().unwrap();
        s.max_iterations = 30;
        d.save_settings(&s).unwrap();
        d.with_conn(|c| {
            c.execute_batch(include_str!("../migrations/004_raise_iteration_limit.sql"))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(d.get_settings().unwrap().max_iterations, 100);
        // Custom values are kept.
        s.max_iterations = 45;
        d.save_settings(&s).unwrap();
        d.with_conn(|c| {
            c.execute_batch(include_str!("../migrations/004_raise_iteration_limit.sql"))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(d.get_settings().unwrap().max_iterations, 45);
    }

    #[test]
    fn settings_roundtrip_and_defaults() {
        let d = db();
        let mut s = d.get_settings().unwrap();
        assert_eq!(s.theme, "dark");
        s.theme = "light".into();
        s.max_iterations = 12;
        d.save_settings(&s).unwrap();
        let s2 = d.get_settings().unwrap();
        assert_eq!(s2.theme, "light");
        assert_eq!(s2.max_iterations, 12);
    }

    #[test]
    fn retention_deletes_old_sessions() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p4", "p4").unwrap();
        let s = d.create_session(&w.id, "old", None, None, None).unwrap();
        // Force updated_at far into the past.
        d.with_conn(|c| {
            c.execute("UPDATE sessions SET updated_at = 1 WHERE id = ?1", params![s.id])
                .map_err(Into::into)
        })
        .unwrap();
        let removed = d.apply_retention(7).unwrap();
        assert_eq!(removed, 1);
        assert!(d.list_sessions(None).unwrap().is_empty());
    }

    #[test]
    fn export_history_contains_messages() {
        let d = db();
        let w = d.upsert_workspace("/tmp/p5", "p5").unwrap();
        let s = d.create_session(&w.id, "s", None, None, None).unwrap();
        d.append_message(&MessageRecord {
            id: new_id(),
            session_id: s.id,
            role: Role::Assistant,
            content: "hi there".into(),
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
            reasoning: None,
            created_at: now_ms(),
        })
        .unwrap();
        let json = d.export_history_json().unwrap();
        assert!(json.contains("hi there"));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["messages"].as_array().unwrap().len() == 1);
    }
}
