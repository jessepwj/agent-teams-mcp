//! Dependency graph for tasks.
//!
//! Operates on a snapshot of `TaskFile` values to detect cycles,
//! compute topological order, and query dependency relationships.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::{Error, Result};
use crate::models::task::{TaskFile, TaskStatus};

/// A directed acyclic graph representing task dependencies.
///
/// Edges go from a task to the tasks it depends on (its `blocked_by` set).
/// For example, if task B is blocked by task A, we store `B -> {A}`.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// task_id -> set of task_ids it depends on (blocked_by)
    deps: HashMap<String, HashSet<String>>,
    /// Reverse index: task_id -> set of task_ids it blocks
    rdeps: HashMap<String, HashSet<String>>,
}

impl DependencyGraph {
    /// Build a dependency graph from a slice of tasks.
    pub fn from_tasks(tasks: &[TaskFile]) -> Self {
        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
        let mut rdeps: HashMap<String, HashSet<String>> = HashMap::new();

        for task in tasks {
            // Ensure every task has an entry even if it has no deps
            deps.entry(task.id.clone()).or_default();
            rdeps.entry(task.id.clone()).or_default();

            for dep in &task.blocked_by {
                deps.entry(task.id.clone()).or_default().insert(dep.clone());
                rdeps
                    .entry(dep.clone())
                    .or_default()
                    .insert(task.id.clone());
            }
        }

        Self { deps, rdeps }
    }

    /// Check whether adding an edge `task_id -> depends_on` would create a cycle.
    ///
    /// Uses BFS starting from `depends_on`, following existing dependency edges.
    /// If we can reach `task_id` from `depends_on`, adding this edge would
    /// create a cycle.
    pub fn would_create_cycle(&self, task_id: &str, depends_on: &str) -> bool {
        if task_id == depends_on {
            return true;
        }

        // BFS from depends_on, following its own dependencies
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(depends_on.to_string());
        visited.insert(depends_on.to_string());

        while let Some(current) = queue.pop_front() {
            if let Some(current_deps) = self.deps.get(&current) {
                for dep in current_deps {
                    if dep == task_id {
                        return true;
                    }
                    if visited.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        false
    }

    /// Add a dependency edge: `task_id` is now blocked by `depends_on`.
    ///
    /// Returns an error if this would create a cycle.
    pub fn add_dependency(&mut self, task_id: &str, depends_on: &str) -> Result<()> {
        if self.would_create_cycle(task_id, depends_on) {
            return Err(Error::CycleDetected {
                from: task_id.to_string(),
                to: depends_on.to_string(),
            });
        }

        self.deps
            .entry(task_id.to_string())
            .or_default()
            .insert(depends_on.to_string());
        self.rdeps
            .entry(depends_on.to_string())
            .or_default()
            .insert(task_id.to_string());

        Ok(())
    }

    /// Get the list of task IDs that `task_id` is blocked by.
    pub fn get_blocked_by(&self, task_id: &str) -> Vec<String> {
        self.deps
            .get(task_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the list of task IDs that `task_id` blocks.
    pub fn get_blocks(&self, task_id: &str) -> Vec<String> {
        self.rdeps
            .get(task_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Check whether a task is ready to run (all its dependencies are completed or deleted).
    pub fn is_ready(&self, task_id: &str, tasks: &[TaskFile]) -> bool {
        let blocked_by = match self.deps.get(task_id) {
            Some(deps) if !deps.is_empty() => deps,
            _ => return true,
        };

        let status_map: HashMap<&str, TaskStatus> =
            tasks.iter().map(|t| (t.id.as_str(), t.status)).collect();

        blocked_by.iter().all(|dep_id| {
            matches!(
                status_map.get(dep_id.as_str()),
                Some(TaskStatus::Completed) | Some(TaskStatus::Deleted)
            )
        })
    }

    // -----------------------------------------------------------------------
    // Visualization & analysis
    // -----------------------------------------------------------------------

    /// Export the dependency graph as a [Mermaid](https://mermaid.js.org/) diagram.
    ///
    /// Nodes are colored by status:
    /// - Pending: light gray
    /// - InProgress: light blue
    /// - Completed: light green
    /// - Deleted: light pink
    ///
    /// Edges represent `blocked_by` relationships: `dep --> task`.
    pub fn to_mermaid(&self, tasks: &[TaskFile]) -> String {
        let mut out = String::from("graph TD\n");

        let task_map: HashMap<&str, &TaskFile> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        // Sort node IDs for deterministic output
        let mut node_ids: Vec<&str> = self.deps.keys().map(|s| s.as_str()).collect();
        node_ids.sort();

        for id in &node_ids {
            if let Some(task) = task_map.get(id) {
                let escaped_subject = task.subject.replace('"', "#quot;");
                let color = status_color(task.status);
                out.push_str(&format!(
                    "    {id}[\"{escaped_subject}\"]\n    style {id} fill:{color}\n"
                ));
            }
        }

        // Edges: for each task, draw dep --> task for each blocked_by
        for id in &node_ids {
            if let Some(deps) = self.deps.get(*id) {
                let mut sorted_deps: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
                sorted_deps.sort();
                for dep in sorted_deps {
                    out.push_str(&format!("    {dep} --> {id}\n"));
                }
            }
        }

        out
    }

    /// Export the dependency graph as a [Graphviz DOT](https://graphviz.org/) diagram.
    ///
    /// Same information as [`to_mermaid`](Self::to_mermaid) but in DOT syntax.
    pub fn to_dot(&self, tasks: &[TaskFile]) -> String {
        let mut out = String::from("digraph tasks {\n    rankdir=LR;\n");

        let task_map: HashMap<&str, &TaskFile> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        let mut node_ids: Vec<&str> = self.deps.keys().map(|s| s.as_str()).collect();
        node_ids.sort();

        for id in &node_ids {
            if let Some(task) = task_map.get(id) {
                let escaped_subject = task.subject.replace('"', "\\\"");
                let color = status_color_dot(task.status);
                out.push_str(&format!(
                    "    {id} [label=\"{escaped_subject}\" style=filled fillcolor=\"{color}\"];\n"
                ));
            }
        }

        for id in &node_ids {
            if let Some(deps) = self.deps.get(*id) {
                let mut sorted_deps: Vec<&str> = deps.iter().map(|s| s.as_str()).collect();
                sorted_deps.sort();
                for dep in sorted_deps {
                    out.push_str(&format!("    {dep} -> {id};\n"));
                }
            }
        }

        out.push_str("}\n");
        out
    }

    /// Compute the critical path (longest dependency chain) through the graph.
    ///
    /// Uses topological sort + dynamic programming:
    /// `longest[node] = max(longest[dep] + 1)` for each dependency.
    ///
    /// Deleted tasks are excluded from the computation.
    /// Returns task IDs in dependency-first order (root → leaf).
    pub fn critical_path(&self, tasks: &[TaskFile]) -> Vec<String> {
        let status_map: HashMap<&str, TaskStatus> =
            tasks.iter().map(|t| (t.id.as_str(), t.status)).collect();

        // Filter out deleted tasks
        let active_ids: HashSet<&str> = self
            .deps
            .keys()
            .map(|s| s.as_str())
            .filter(|id| !matches!(status_map.get(id), Some(TaskStatus::Deleted)))
            .collect();

        if active_ids.is_empty() {
            return Vec::new();
        }

        // Topological sort on active tasks only
        // Compute in-degree considering only active tasks
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for &id in &active_ids {
            in_degree.insert(id, 0);
        }
        for &id in &active_ids {
            if let Some(deps) = self.deps.get(id) {
                let active_dep_count = deps
                    .iter()
                    .filter(|d| active_ids.contains(d.as_str()))
                    .count();
                in_degree.insert(id, active_dep_count);
            }
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();
        let mut sorted_queue: Vec<&str> = queue.drain(..).collect();
        sorted_queue.sort();
        queue.extend(sorted_queue);

        let mut topo_order: Vec<&str> = Vec::new();
        while let Some(node) = queue.pop_front() {
            topo_order.push(node);
            if let Some(blocked) = self.rdeps.get(node) {
                let mut next: Vec<&str> = blocked
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|s| active_ids.contains(s))
                    .collect();
                next.sort();
                for dep in next {
                    if let Some(deg) = in_degree.get_mut(dep) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        // DP: compute longest path ending at each node
        let mut longest: HashMap<&str, usize> = HashMap::new();
        let mut predecessor: HashMap<&str, &str> = HashMap::new();

        for &node in &topo_order {
            longest.insert(node, 1); // minimum path length = 1 (the node itself)
        }

        for &node in &topo_order {
            if let Some(blocked) = self.rdeps.get(node) {
                for dependent in blocked.iter().map(|s| s.as_str()) {
                    if !active_ids.contains(dependent) {
                        continue;
                    }
                    let new_len = longest[node] + 1;
                    if new_len > *longest.get(dependent).unwrap_or(&0) {
                        longest.insert(dependent, new_len);
                        predecessor.insert(dependent, node);
                    }
                }
            }
        }

        // Find the node with the longest path
        let end_node = match longest.iter().max_by_key(|&(_, &len)| len) {
            Some((&node, _)) => node,
            None => return Vec::new(),
        };

        // Backtrack to reconstruct the path
        let mut path = vec![end_node];
        let mut current = end_node;
        while let Some(&prev) = predecessor.get(current) {
            path.push(prev);
            current = prev;
        }

        path.reverse();
        path.into_iter().map(String::from).collect()
    }

    /// Render the DAG as a terminal-friendly Unicode diagram with ANSI colors.
    ///
    /// Groups tasks by topological level (phase) and draws connectors.
    /// Status is shown with colored symbols:
    /// - `○` Pending (gray), `◉` InProgress (blue), `●` Completed (green), `✗` Deleted (red)
    ///
    /// ```text
    /// Task DAG ─────────────────────────────────
    ///
    ///   Phase 0
    ///   ├── ● #1  CC: Concurrency analysis
    ///   ├── ● #2  Codex: Security audit
    ///   └── ● #3  Gemini: API design review
    ///        │
    ///        ▼
    ///   Phase 1
    ///   ├── ◉ #4  Codex: Fix proposals        ← #1, #2
    ///   └── ○ #5  Gemini: Refactoring         ← #2, #3
    ///        │
    ///        ▼
    ///   Phase 2
    ///   └── ○ #6  CC: Final synthesis         ← #4, #5
    /// ```
    pub fn to_terminal(&self, tasks: &[TaskFile]) -> String {
        let task_map: HashMap<&str, &TaskFile> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

        // Compute topological levels via BFS
        let levels = self.compute_levels();

        if levels.is_empty() {
            return String::from("  (empty DAG)\n");
        }

        // Group task IDs by level, sorted within each group
        let max_level = levels.values().copied().max().unwrap_or(0);
        let mut level_groups: Vec<Vec<&str>> = vec![Vec::new(); max_level + 1];
        for (id, &level) in &levels {
            level_groups[level].push(id.as_str());
        }
        for group in &mut level_groups {
            group.sort();
        }

        // Find the critical path for highlighting
        let cp: HashSet<String> = self.critical_path(tasks).into_iter().collect();

        let mut out = String::new();

        // Header
        let total = tasks.len();
        let completed = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .count();
        let in_progress = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        out.push_str(&format!(
            "  Task DAG  \x1b[90m({total} tasks: {completed} done, {in_progress} active)\x1b[0m\n"
        ));
        out.push_str("  ─────────────────────────────────────────────\n");

        for (level, group) in level_groups.iter().enumerate() {
            if group.is_empty() {
                continue;
            }

            // Phase header
            out.push_str(&format!("\n  \x1b[1mPhase {level}\x1b[0m\n"));

            for (i, &id) in group.iter().enumerate() {
                let is_last = i == group.len() - 1;
                let connector = if is_last { "└──" } else { "├──" };

                if let Some(task) = task_map.get(id) {
                    let (symbol, color) = status_symbol_ansi(task.status);
                    let on_cp = if cp.contains(id) {
                        " \x1b[33m★\x1b[0m"
                    } else {
                        ""
                    };

                    // Show blocked_by as dependency hint
                    let deps_hint = if task.blocked_by.is_empty() {
                        String::new()
                    } else {
                        let dep_ids: Vec<&str> =
                            task.blocked_by.iter().map(|s| s.as_str()).collect();
                        format!("  \x1b[90m← {}\x1b[0m", dep_ids.join(", "))
                    };

                    // Owner hint
                    let owner_hint = task
                        .owner
                        .as_deref()
                        .map(|o| format!("  \x1b[90m@{o}\x1b[0m"))
                        .unwrap_or_default();

                    out.push_str(&format!(
                        "  {connector} {color}{symbol}\x1b[0m \x1b[1m#{id}\x1b[0m  {subject}{on_cp}{deps_hint}{owner_hint}\n",
                        subject = task.subject,
                    ));
                }
            }

            // Draw connector to next phase (if there is one)
            if level < max_level {
                out.push_str("       │\n");
                out.push_str("       ▼\n");
            }
        }

        // Critical path summary
        if cp.len() > 1 {
            let cp_ordered = self.critical_path(tasks);
            let cp_str: Vec<String> = cp_ordered.iter().map(|id| format!("#{id}")).collect();
            out.push_str(&format!(
                "\n  \x1b[33m★ Critical path:\x1b[0m {}\n",
                cp_str.join(" → ")
            ));
        }

        out
    }

    /// Render the DAG as a plain-text terminal diagram (no ANSI colors).
    ///
    /// Identical layout to [`to_terminal`](Self::to_terminal) but without
    /// escape sequences, suitable for logging or file output.
    pub fn to_terminal_plain(&self, tasks: &[TaskFile]) -> String {
        // Strip ANSI escape sequences from the colored version
        let colored = self.to_terminal(tasks);
        strip_ansi(&colored)
    }

    /// Compute the topological level of each node.
    ///
    /// Level 0 = roots (no dependencies); level N = max(level of deps) + 1.
    fn compute_levels(&self) -> HashMap<String, usize> {
        let mut levels: HashMap<String, usize> = HashMap::new();

        // Topologically sorted order ensures deps are processed first
        if let Ok(topo) = self.topological_sort() {
            for id in &topo {
                let level = if let Some(deps) = self.deps.get(id) {
                    deps.iter()
                        .filter_map(|d| levels.get(d))
                        .max()
                        .map(|max_dep| max_dep + 1)
                        .unwrap_or(0)
                } else {
                    0
                };
                levels.insert(id.clone(), level);
            }
        }

        levels
    }

    /// Compute a topological ordering of the tasks (Kahn's algorithm).
    ///
    /// Returns task IDs in an order where dependencies come before dependents.
    /// Returns an error if the graph contains a cycle.
    pub fn topological_sort(&self) -> Result<Vec<String>> {
        // Compute in-degree for each node
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for node in self.deps.keys() {
            in_degree.entry(node.clone()).or_insert(0);
        }
        for node in self.rdeps.keys() {
            in_degree.entry(node.clone()).or_insert(0);
        }
        // in_degree[x] = number of tasks x depends on
        for (node, node_deps) in &self.deps {
            *in_degree.entry(node.clone()).or_insert(0) = node_deps.len();
        }

        // Start with nodes that have no dependencies (in-degree 0)
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|&(_, deg)| *deg == 0)
            .map(|(id, _)| id.clone())
            .collect();

        // Sort for deterministic output
        let mut sorted_queue: Vec<String> = queue.drain(..).collect();
        sorted_queue.sort();
        queue.extend(sorted_queue);

        let mut result = Vec::new();

        while let Some(node) = queue.pop_front() {
            result.push(node.clone());

            // For each task that this node blocks, decrement its in-degree
            if let Some(blocked) = self.rdeps.get(&node) {
                let mut next: Vec<&String> = blocked.iter().collect();
                next.sort(); // deterministic ordering
                for dependent in next {
                    if let Some(deg) = in_degree.get_mut(dependent) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        if result.len() != in_degree.len() {
            // There's a cycle -- find a task involved for a useful error message
            let in_cycle: Vec<_> = in_degree
                .iter()
                .filter(|&(_, deg)| *deg > 0)
                .map(|(id, _)| id.clone())
                .collect();
            return Err(Error::CycleDetected {
                from: in_cycle.first().cloned().unwrap_or_default(),
                to: "...".into(),
            });
        }

        Ok(result)
    }
}

/// Map a task status to a Mermaid-compatible fill color.
fn status_color(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "#D3D3D3",
        TaskStatus::InProgress => "#87CEEB",
        TaskStatus::Completed => "#90EE90",
        TaskStatus::Deleted => "#FFB6C1",
    }
}

/// Map a task status to a Graphviz DOT fillcolor.
fn status_color_dot(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "#D3D3D3",
        TaskStatus::InProgress => "#87CEEB",
        TaskStatus::Completed => "#90EE90",
        TaskStatus::Deleted => "#FFB6C1",
    }
}

/// Map a task status to a Unicode symbol and ANSI color code.
///
/// Returns `(symbol, ansi_prefix)` — caller must append `\x1b[0m` to reset.
fn status_symbol_ansi(status: TaskStatus) -> (&'static str, &'static str) {
    match status {
        TaskStatus::Pending => ("○", "\x1b[90m"),    // gray
        TaskStatus::InProgress => ("◉", "\x1b[34m"), // blue
        TaskStatus::Completed => ("●", "\x1b[32m"),  // green
        TaskStatus::Deleted => ("✗", "\x1b[31m"),    // red
    }
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::task::TaskFile;

    fn make_task(id: &str, blocked_by: Vec<&str>) -> TaskFile {
        TaskFile {
            id: id.to_string(),
            subject: format!("Task {id}"),
            description: None,
            active_form: None,
            status: TaskStatus::Pending,
            owner: None,
            blocks: vec![],
            blocked_by: blocked_by.into_iter().map(String::from).collect(),
            metadata: None,
        }
    }

    fn make_task_with_status(id: &str, status: TaskStatus, blocked_by: Vec<&str>) -> TaskFile {
        let mut t = make_task(id, blocked_by);
        t.status = status;
        t
    }

    #[test]
    fn simple_dag() {
        // A -> B -> C (C blocked_by B, B blocked_by A)
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        assert_eq!(graph.get_blocked_by("C"), vec!["B".to_string()]);
        assert_eq!(graph.get_blocked_by("B"), vec!["A".to_string()]);
        assert!(graph.get_blocked_by("A").is_empty());

        assert!(graph.get_blocks("A").contains(&"B".to_string()));
        assert!(graph.get_blocks("B").contains(&"C".to_string()));
        assert!(graph.get_blocks("C").is_empty());
    }

    #[test]
    fn cycle_detection_direct() {
        // A -> B (B depends on A)
        let tasks = vec![make_task("A", vec![]), make_task("B", vec!["A"])];
        let graph = DependencyGraph::from_tasks(&tasks);

        // Adding A depends on B would create A -> B -> A
        assert!(graph.would_create_cycle("A", "B"));
        // Adding C depends on A is fine (C doesn't exist in graph yet, but that's ok)
        assert!(!graph.would_create_cycle("C", "A"));
    }

    #[test]
    fn cycle_detection_indirect() {
        // A -> B -> C
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        // Adding A depends on C would create A -> B -> C -> A
        assert!(graph.would_create_cycle("A", "C"));
    }

    #[test]
    fn self_reference_is_cycle() {
        let tasks = vec![make_task("A", vec![])];
        let graph = DependencyGraph::from_tasks(&tasks);

        assert!(graph.would_create_cycle("A", "A"));
    }

    #[test]
    fn diamond_dependencies() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        // D blocked_by B and C; B and C blocked_by A
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["A"]),
            make_task("D", vec!["B", "C"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        let d_deps = graph.get_blocked_by("D");
        assert!(d_deps.contains(&"B".to_string()));
        assert!(d_deps.contains(&"C".to_string()));
        assert_eq!(d_deps.len(), 2);

        // No cycles in a diamond
        assert!(!graph.would_create_cycle("D", "A"));
        // But A depending on D would be a cycle
        assert!(graph.would_create_cycle("A", "D"));
    }

    #[test]
    fn add_dependency_ok() {
        let tasks = vec![make_task("A", vec![]), make_task("B", vec![])];
        let mut graph = DependencyGraph::from_tasks(&tasks);

        graph.add_dependency("B", "A").unwrap();
        assert!(graph.get_blocked_by("B").contains(&"A".to_string()));
    }

    #[test]
    fn add_dependency_cycle_rejected() {
        let tasks = vec![make_task("A", vec![]), make_task("B", vec!["A"])];
        let mut graph = DependencyGraph::from_tasks(&tasks);

        let err = graph.add_dependency("A", "B").unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn is_ready_no_deps() {
        let tasks = vec![make_task("A", vec![])];
        let graph = DependencyGraph::from_tasks(&tasks);

        assert!(graph.is_ready("A", &tasks));
    }

    #[test]
    fn is_ready_deps_completed() {
        let tasks = vec![
            make_task_with_status("A", TaskStatus::Completed, vec![]),
            make_task("B", vec!["A"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        assert!(graph.is_ready("B", &tasks));
    }

    #[test]
    fn is_ready_deps_pending() {
        let tasks = vec![make_task("A", vec![]), make_task("B", vec!["A"])];
        let graph = DependencyGraph::from_tasks(&tasks);

        assert!(!graph.is_ready("B", &tasks));
    }

    #[test]
    fn is_ready_deps_deleted() {
        let tasks = vec![
            make_task_with_status("A", TaskStatus::Deleted, vec![]),
            make_task("B", vec!["A"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        assert!(graph.is_ready("B", &tasks));
    }

    #[test]
    fn topological_sort_linear() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        let order = graph.topological_sort().unwrap();
        assert_eq!(order, vec!["A", "B", "C"]);
    }

    #[test]
    fn topological_sort_diamond() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["A"]),
            make_task("D", vec!["B", "C"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        let order = graph.topological_sort().unwrap();
        // A must come first, D must come last, B and C can be in either order
        assert_eq!(order[0], "A");
        assert_eq!(order[3], "D");
        assert!(order[1] == "B" || order[1] == "C");
        assert!(order[2] == "B" || order[2] == "C");
    }

    #[test]
    fn topological_sort_independent() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec![]),
            make_task("C", vec![]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);

        let order = graph.topological_sort().unwrap();
        // All independent -- should be sorted alphabetically for determinism
        assert_eq!(order, vec!["A", "B", "C"]);
    }

    // -----------------------------------------------------------------------
    // Mermaid visualization tests
    // -----------------------------------------------------------------------

    #[test]
    fn mermaid_simple_chain() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let mermaid = graph.to_mermaid(&tasks);

        assert!(mermaid.starts_with("graph TD\n"));
        assert!(mermaid.contains("A[\"Task A\"]"));
        assert!(mermaid.contains("B[\"Task B\"]"));
        assert!(mermaid.contains("C[\"Task C\"]"));
        assert!(mermaid.contains("A --> B"));
        assert!(mermaid.contains("B --> C"));
    }

    #[test]
    fn mermaid_with_status_colors() {
        let tasks = vec![
            make_task_with_status("A", TaskStatus::Completed, vec![]),
            make_task_with_status("B", TaskStatus::InProgress, vec!["A"]),
            make_task_with_status("C", TaskStatus::Pending, vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let mermaid = graph.to_mermaid(&tasks);

        assert!(
            mermaid.contains("fill:#90EE90"),
            "Completed should be green"
        );
        assert!(
            mermaid.contains("fill:#87CEEB"),
            "InProgress should be blue"
        );
        assert!(mermaid.contains("fill:#D3D3D3"), "Pending should be gray");
    }

    #[test]
    fn mermaid_empty_graph() {
        let tasks: Vec<TaskFile> = vec![];
        let graph = DependencyGraph::from_tasks(&tasks);
        let mermaid = graph.to_mermaid(&tasks);

        assert_eq!(mermaid, "graph TD\n");
    }

    #[test]
    fn mermaid_escapes_quotes() {
        let mut task = make_task("A", vec![]);
        task.subject = r#"Task with "quotes""#.to_string();
        let tasks = vec![task];
        let graph = DependencyGraph::from_tasks(&tasks);
        let mermaid = graph.to_mermaid(&tasks);

        // Double quotes should be escaped so Mermaid doesn't break
        assert!(mermaid.contains("#quot;"), "Quotes should be escaped");
        assert!(!mermaid.contains(r#"[""#) || mermaid.contains("#quot;"));
    }

    // -----------------------------------------------------------------------
    // DOT visualization tests
    // -----------------------------------------------------------------------

    #[test]
    fn dot_simple_chain() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let dot = graph.to_dot(&tasks);

        assert!(dot.starts_with("digraph tasks {"));
        assert!(dot.contains("rankdir=LR"));
        assert!(dot.contains("A [label=\"Task A\""));
        assert!(dot.contains("A -> B;"));
        assert!(dot.contains("B -> C;"));
        assert!(dot.ends_with("}\n"));
    }

    // -----------------------------------------------------------------------
    // Critical path tests
    // -----------------------------------------------------------------------

    #[test]
    fn critical_path_linear() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let path = graph.critical_path(&tasks);

        assert_eq!(path, vec!["A", "B", "C"]);
    }

    #[test]
    fn critical_path_diamond() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        // Both A→B→D and A→C→D are length 3.
        // The algorithm picks one deterministically.
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec!["A"]),
            make_task("C", vec!["A"]),
            make_task("D", vec!["B", "C"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let path = graph.critical_path(&tasks);

        assert_eq!(path.len(), 3);
        assert_eq!(path[0], "A");
        assert_eq!(path[2], "D");
        // Middle element is either B or C
        assert!(path[1] == "B" || path[1] == "C");
    }

    #[test]
    fn critical_path_independent() {
        let tasks = vec![
            make_task("A", vec![]),
            make_task("B", vec![]),
            make_task("C", vec![]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let path = graph.critical_path(&tasks);

        // All independent, so critical path is a single task
        assert_eq!(path.len(), 1);
    }

    #[test]
    fn critical_path_skips_deleted() {
        // A → B(deleted) → C
        // With B deleted, A and C are independent; critical path = 1
        let tasks = vec![
            make_task("A", vec![]),
            make_task_with_status("B", TaskStatus::Deleted, vec!["A"]),
            make_task("C", vec!["B"]),
        ];
        let graph = DependencyGraph::from_tasks(&tasks);
        let path = graph.critical_path(&tasks);

        // B is deleted, so the chain is broken
        assert!(!path.contains(&"B".to_string()));
        // Path should be shorter since the chain is broken
        assert!(path.len() <= 2);
    }

    // -----------------------------------------------------------------------
    // Terminal rendering tests
    // -----------------------------------------------------------------------

    #[test]
    fn terminal_diamond_dag() {
        let tasks = vec![
            make_task_with_status("1", TaskStatus::Completed, vec![]),
            make_task_with_status("2", TaskStatus::Completed, vec![]),
            make_task_with_status("3", TaskStatus::Completed, vec![]),
            make_task_with_status("4", TaskStatus::InProgress, vec!["1", "2"]),
            make_task_with_status("5", TaskStatus::InProgress, vec!["2", "3"]),
            make_task("6", vec!["4", "5"]),
        ];
        // Give them real subjects
        let mut tasks = tasks;
        tasks[0].subject = "CC: Concurrency".into();
        tasks[1].subject = "Codex: Security".into();
        tasks[2].subject = "Gemini: API".into();
        tasks[3].subject = "Codex: Fixes".into();
        tasks[4].subject = "Gemini: Refactor".into();
        tasks[5].subject = "CC: Synthesis".into();

        let graph = DependencyGraph::from_tasks(&tasks);
        let output = graph.to_terminal(&tasks);

        // Should contain all three phases
        assert!(output.contains("Phase 0"));
        assert!(output.contains("Phase 1"));
        assert!(output.contains("Phase 2"));

        // Should contain status symbols
        assert!(output.contains("●"), "Completed should show ●");
        assert!(output.contains("◉"), "InProgress should show ◉");
        assert!(output.contains("○"), "Pending should show ○");

        // Should contain task subjects
        assert!(output.contains("CC: Concurrency"));
        assert!(output.contains("CC: Synthesis"));

        // Should contain dependency hints
        assert!(output.contains("← 1, 2"));
        assert!(output.contains("← 4, 5"));

        // Should contain critical path
        assert!(output.contains("Critical path"));

        // Should contain phase connectors
        assert!(output.contains("▼"));
    }

    #[test]
    fn terminal_plain_strips_ansi() {
        let tasks = vec![make_task("A", vec![]), make_task("B", vec!["A"])];
        let graph = DependencyGraph::from_tasks(&tasks);
        let plain = graph.to_terminal_plain(&tasks);

        // Should not contain any ANSI escape sequences
        assert!(
            !plain.contains("\x1b"),
            "Plain output should have no ANSI escapes"
        );
        // But should still contain the content
        assert!(plain.contains("Phase 0"));
        assert!(plain.contains("Task A"));
    }

    #[test]
    fn terminal_empty_graph() {
        let tasks: Vec<TaskFile> = vec![];
        let graph = DependencyGraph::from_tasks(&tasks);
        let output = graph.to_terminal(&tasks);

        assert!(output.contains("empty DAG"));
    }

    #[test]
    fn terminal_single_task() {
        let tasks = vec![make_task("1", vec![])];
        let graph = DependencyGraph::from_tasks(&tasks);
        let output = graph.to_terminal(&tasks);

        assert!(output.contains("Phase 0"));
        assert!(output.contains("#1"));
        // No critical path for single task
        assert!(!output.contains("Critical path"));
    }

    #[test]
    fn terminal_with_owners() {
        let mut tasks = vec![make_task("1", vec![])];
        tasks[0].owner = Some("alice".into());
        let graph = DependencyGraph::from_tasks(&tasks);
        let plain = graph.to_terminal_plain(&tasks);

        assert!(plain.contains("@alice"));
    }

    #[test]
    fn strip_ansi_works() {
        let input = "\x1b[32m●\x1b[0m hello \x1b[1mbold\x1b[0m";
        let stripped = strip_ansi(input);
        assert_eq!(stripped, "● hello bold");
    }
}
