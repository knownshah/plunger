//! Workflows and the record of their runs.

use super::*;

pub use crate::store::log::WORKFLOW_RUNS_KEPT;

/// A workflow run to record.
pub struct NewWorkflowRun<'a> {
    pub workflow: &'a str,
    pub started_at: &'a str,
    pub source: Source,
    pub ok: bool,
    pub cancelled: bool,
    /// The per-step results as JSON.
    pub steps: &'a str,
}

/// A recorded workflow run.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowRunRow {
    pub id: i64,
    pub workflow: String,
    pub started_at: String,
    pub source: Source,
    pub ok: bool,
    pub cancelled: bool,
    /// The per-step results as JSON.
    pub steps: String,
}

fn workflow_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkflowRunRow> {
    Ok(WorkflowRunRow {
        id: row.get(0)?,
        workflow: row.get(1)?,
        started_at: row.get(2)?,
        source: Source::parse(&row.get::<_, String>(3)?),
        ok: row.get(4)?,
        cancelled: row.get(5)?,
        steps: row.get(6)?,
    })
}

impl History {
    /// Creates or replaces a workflow; `steps` is its JSON.
    pub fn save_workflow(&self, name: &str, steps: &str) -> rusqlite::Result<()> {
        self.commit(Event::WorkflowSaved { name: name.to_string(), steps: steps.to_string(), updated_at: now() })?;
        Ok(())
    }

    /// Records one run of a workflow, keeping the newest `WORKFLOW_RUNS_KEPT` of each workflow.
    pub fn record_workflow_run(&self, run: &NewWorkflowRun) -> rusqlite::Result<i64> {
        let done = self.commit(Event::WorkflowRunRecorded {
            id: 0, // handed out by `commit`
            workflow: run.workflow.to_string(),
            started_at: run.started_at.to_string(),
            source: run.source.as_str().to_string(),
            ok: run.ok,
            cancelled: run.cancelled,
            steps: run.steps.to_string(),
        })?;
        Ok(done.id)
    }

    /// The runs of one workflow, newest first.
    pub fn workflow_runs(&self, workflow: &str, limit: i64) -> rusqlite::Result<Vec<WorkflowRunRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workflow, started_at, source, ok, cancelled, steps FROM workflow_runs WHERE workflow = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![workflow, limit], workflow_run_from_row)?;
        rows.collect()
    }

    /// The newest run of every workflow that has one.
    pub fn latest_workflow_runs(&self) -> rusqlite::Result<Vec<WorkflowRunRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, workflow, started_at, source, ok, cancelled, steps FROM workflow_runs
             WHERE id IN (SELECT MAX(id) FROM workflow_runs GROUP BY workflow)",
        )?;
        let rows = stmt.query_map([], workflow_run_from_row)?;
        rows.collect()
    }

    /// A workflow's steps (JSON), if there is one by that name.
    pub fn get_workflow(&self, name: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT steps FROM workflows WHERE name = ?1", params![name], |r| r.get(0))
            .optional()
    }

    /// Workflows by name, with their steps (JSON).
    pub fn list_workflows(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT name, steps FROM workflows ORDER BY name COLLATE NOCASE, name")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// Files a workflow under `group` (None takes it out of its group); false when there is no such workflow.
    pub fn set_workflow_group(&self, name: &str, group: Option<&str>) -> rusqlite::Result<bool> {
        if self.get_workflow(name)?.is_none() {
            return Ok(false);
        }
        Ok(self.commit(Event::WorkflowGrouped { name: name.to_string(), group: group.map(str::to_string) })?.changed > 0)
    }

    /// The group of each workflow that has one, by name.
    pub fn workflow_groups(&self) -> rusqlite::Result<std::collections::BTreeMap<String, String>> {
        let mut stmt = self.conn.prepare("SELECT name, group_name FROM workflows WHERE group_name IS NOT NULL")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    }

    /// Removes a workflow and its recorded runs; false when there was none by that name.
    pub fn delete_workflow(&self, name: &str) -> rusqlite::Result<bool> {
        if self.get_workflow(name)?.is_none() {
            // no workflow, but its recorded runs go all the same
            let runs: i64 = self.conn.query_row("SELECT COUNT(*) FROM workflow_runs WHERE workflow = ?1", params![name], |r| r.get(0))?;
            if runs > 0 {
                self.commit(Event::WorkflowDeleted { name: name.to_string() })?;
            }
            return Ok(false);
        }
        self.commit(Event::WorkflowDeleted { name: name.to_string() })?;
        Ok(true)
    }
}
