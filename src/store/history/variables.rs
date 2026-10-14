//! Variables an agent set from the command line or over MCP.

use super::*;

/// A variable set from the command line or over MCP.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentVariable {
    pub name: String,
    /// Empty for a secret: its value is in the credential store.
    pub value: String,
    pub secret: bool,
    /// Who set it: `cli` or `mcp`.
    pub source: String,
    pub updated_at: String,
}

impl History {
    /// Creates or updates a variable set by an agent. A secret's value must not be passed here
    /// (it goes to the credential store); only the fact that it exists is recorded.
    pub fn set_agent_variable(&self, name: &str, value: &str, secret: bool, source: Source) -> rusqlite::Result<()> {
        self.commit(Event::VariableSet {
            name: name.to_string(),
            value: value.to_string(),
            secret,
            source: source.as_str().to_string(),
            updated_at: now(),
        })?;
        Ok(())
    }

    /// Variables set by agents, by name.
    pub fn list_agent_variables(&self) -> rusqlite::Result<Vec<AgentVariable>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, value, secret, source, updated_at FROM agent_variables ORDER BY name COLLATE NOCASE, name")?;
        let rows = stmt.query_map([], |r| {
            Ok(AgentVariable { name: r.get(0)?, value: r.get(1)?, secret: r.get(2)?, source: r.get(3)?, updated_at: r.get(4)? })
        })?;
        rows.collect()
    }

    /// Removes one agent variable; false when there was none by that name.
    pub fn delete_agent_variable(&self, name: &str) -> rusqlite::Result<bool> {
        let exists: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_variables WHERE name = ?1)", params![name], |r| r.get(0))?;
        if !exists {
            return Ok(false);
        }
        Ok(self.commit(Event::VariableDeleted { name: name.to_string() })?.changed > 0)
    }

    /// Removes every agent variable and returns their names (so the caller can forget secrets).
    pub fn clear_agent_variables(&self) -> rusqlite::Result<Vec<String>> {
        let names: Vec<String> = self.list_agent_variables()?.into_iter().map(|v| v.name).collect();
        if !names.is_empty() {
            self.commit(Event::VariablesCleared)?;
        }
        Ok(names)
    }
}
