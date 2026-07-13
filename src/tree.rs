//! The session tree: domain forest + navigable panel.
//!
//! Parentage comes from tangents state (fork edges); everything else (labels,
//! counts, timestamps) comes from the scanned [`SessionInfo`]. A user-assigned
//! name in state overrides the derived title.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use tui_tree_widget::{Tree, TreeItem, TreeState};

use crate::session::{SessionInfo, short_id};
use crate::state::TangentsState;

/// One node in the session forest.
#[derive(Debug, Clone)]
pub struct Node {
    pub session_id: String,
    pub label: String,
    pub message_count: usize,
    /// Retained for future sorting/recency display.
    #[allow(dead_code)]
    pub last_active: Option<DateTime<Utc>>,
    pub depth: usize,
    pub children: Vec<Node>,
}

/// Colour cycles with depth so nested branches are visually distinct.
pub fn depth_color(depth: usize) -> Color {
    const PALETTE: [Color; 6] = [
        Color::White,
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Magenta,
        Color::Blue,
    ];
    PALETTE[depth % PALETTE.len()]
}

impl Node {
    /// The styled single-line content shown in the tree.
    fn display_line(&self, active: Option<&str>) -> Line<'static> {
        let is_active = active == Some(self.session_id.as_str());
        let mut label_style = Style::default().fg(depth_color(self.depth));
        if is_active {
            label_style = label_style.add_modifier(Modifier::BOLD);
        }
        let mut spans = Vec::new();
        if is_active {
            spans.push(Span::styled("● ", Style::default().fg(Color::Green)));
        }
        spans.push(Span::styled(self.label.clone(), label_style));
        if self.message_count > 0 {
            spans.push(Span::styled(
                format!("  ·{}", self.message_count),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }
}

pub struct TreePanel {
    pub roots: Vec<Node>,
    pub state: TreeState<String>,
    pub visible: bool,
    initialized: bool,
}

impl TreePanel {
    pub fn new(visible: bool) -> Self {
        Self {
            roots: Vec::new(),
            state: TreeState::default(),
            visible,
            initialized: false,
        }
    }

    /// Rebuild the forest from a fresh scan, preserving selection/open state.
    pub fn rebuild(&mut self, sessions: &[SessionInfo], state: &TangentsState) {
        self.roots = build_forest(sessions, state);
        if !self.initialized && !self.roots.is_empty() {
            // First populate: open every node and select the first root.
            for path in open_paths(&self.roots) {
                self.state.open(path);
            }
            self.state.select(vec![self.roots[0].session_id.clone()]);
            self.initialized = true;
        }
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
    }

    pub fn selected_id(&self) -> Option<String> {
        self.state.selected().last().cloned()
    }

    pub fn key_up(&mut self) -> bool {
        self.state.key_up()
    }
    pub fn key_down(&mut self) -> bool {
        self.state.key_down()
    }
    pub fn toggle_selected(&mut self) -> bool {
        self.state.toggle_selected()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool, active: Option<&str>) {
        let items = to_items(&self.roots, active);
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let title = if focused {
            " tangents ◂ "
        } else {
            " tangents "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        // Tree::new only fails on duplicate sibling ids; session ids are unique.
        let tree = match Tree::new(&items) {
            Ok(t) => t,
            Err(_) => return,
        };
        let tree = tree
            .block(block)
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .bg(if focused {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    })
                    .fg(Color::Black),
            )
            .highlight_symbol("▶ ")
            .node_no_children_symbol("· ");
        frame.render_stateful_widget(tree, area, &mut self.state);
    }
}

fn to_items(nodes: &[Node], active: Option<&str>) -> Vec<TreeItem<'static, String>> {
    nodes
        .iter()
        .map(|n| {
            let text = n.display_line(active);
            let children = to_items(&n.children, active);
            if children.is_empty() {
                TreeItem::new_leaf(n.session_id.clone(), text)
            } else {
                // Unwrap is safe: child session ids are globally unique.
                TreeItem::new(n.session_id.clone(), text, children)
                    .expect("unique child identifiers")
            }
        })
        .collect()
}

/// Every node-with-children's full path, for opening on first render.
fn open_paths(nodes: &[Node]) -> Vec<Vec<String>> {
    fn walk(nodes: &[Node], prefix: &[String], out: &mut Vec<Vec<String>>) {
        for n in nodes {
            let mut path = prefix.to_vec();
            path.push(n.session_id.clone());
            if !n.children.is_empty() {
                out.push(path.clone());
                walk(&n.children, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(nodes, &[], &mut out);
    out
}

/// Resolve a session's display label: user name > derived title > short id.
pub fn resolve_label(id: &str, info: Option<&SessionInfo>, state: &TangentsState) -> String {
    state
        .meta(id)
        .and_then(|m| m.name.clone())
        .filter(|n| !n.trim().is_empty())
        .or_else(|| info.map(|i| i.derived_name()))
        .unwrap_or_else(|| format!("session {}", short_id(id)))
}

/// The root→current chain of labels, for the status-bar breadcrumb.
pub fn breadcrumb(sessions: &[SessionInfo], state: &TangentsState, current: &str) -> Vec<String> {
    let infos: HashMap<&str, &SessionInfo> = sessions
        .iter()
        .map(|s| (s.session_id.as_str(), s))
        .collect();
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = Some(current.to_string());
    while let Some(id) = cur {
        if !seen.insert(id.clone()) {
            break; // cycle guard
        }
        chain.push(resolve_label(&id, infos.get(id.as_str()).copied(), state));
        cur = state.meta(&id).and_then(|m| m.parent.clone());
    }
    chain.reverse();
    chain
}

/// Assemble the parent/child forest from sessions + recorded fork edges.
pub fn build_forest(sessions: &[SessionInfo], state: &TangentsState) -> Vec<Node> {
    let infos: HashMap<&str, &SessionInfo> = sessions
        .iter()
        .map(|s| (s.session_id.as_str(), s))
        .collect();

    let archived = |id: &str| state.meta(id).map(|m| m.archived).unwrap_or(false);

    // Resolve a usable parent: recorded, present in scan, and not archived.
    let parent_of = |id: &str| -> Option<String> {
        state
            .meta(id)
            .and_then(|m| m.parent.clone())
            .filter(|p| infos.contains_key(p.as_str()) && !archived(p))
    };

    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    for s in sessions {
        if archived(&s.session_id) {
            continue;
        }
        match parent_of(&s.session_id) {
            Some(p) => children.entry(p).or_default().push(s.session_id.clone()),
            None => roots.push(s.session_id.clone()),
        }
    }

    let sort_key = |id: &str| -> (i64, String) {
        let info = infos.get(id);
        let ts = info
            .and_then(|i| i.created_at.or(i.last_active))
            .map(|d| d.timestamp())
            .unwrap_or(0);
        (ts, id.to_string())
    };
    let sort_ids = |ids: &mut Vec<String>| {
        ids.sort_by_key(|a| sort_key(a));
    };
    sort_ids(&mut roots);

    let mut visited = HashSet::new();
    roots
        .iter()
        .map(|id| build_node(id, 0, &infos, &children, state, &mut visited, &sort_ids))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_node(
    id: &str,
    depth: usize,
    infos: &HashMap<&str, &SessionInfo>,
    children: &HashMap<String, Vec<String>>,
    state: &TangentsState,
    visited: &mut HashSet<String>,
    sort_ids: &dyn Fn(&mut Vec<String>),
) -> Node {
    visited.insert(id.to_string());
    let info = infos.get(id);
    let label = resolve_label(id, info.copied(), state);

    let mut child_nodes = Vec::new();
    if let Some(kids) = children.get(id) {
        let mut kids = kids.clone();
        sort_ids(&mut kids);
        for kid in kids {
            if visited.contains(&kid) {
                continue; // guard against cycles
            }
            child_nodes.push(build_node(
                &kid,
                depth + 1,
                infos,
                children,
                state,
                visited,
                sort_ids,
            ));
        }
    }

    Node {
        session_id: id.to_string(),
        label,
        message_count: info.map(|i| i.message_count).unwrap_or(0),
        last_active: info.and_then(|i| i.last_active),
        depth,
        children: child_nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, count: usize) -> SessionInfo {
        SessionInfo {
            session_id: id.to_string(),
            ai_title: Some(format!("title-{id}")),
            last_prompt: None,
            first_prompt: None,
            message_count: count,
            created_at: None,
            last_active: None,
            git_branch: None,
            version: None,
            first_uuid: None,
            leaf_uuid: None,
        }
    }

    #[test]
    fn forest_nests_recorded_forks() {
        let sessions = vec![info("a", 3), info("b", 1), info("c", 2)];
        let dir = std::env::temp_dir().join(format!("tg-tree-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = TangentsState::load(&dir);
        st.record_fork("b", "a"); // b is a child of a
        let forest = build_forest(&sessions, &st);
        // roots: a and c
        assert_eq!(forest.len(), 2);
        let a = forest.iter().find(|n| n.session_id == "a").unwrap();
        assert_eq!(a.children.len(), 1);
        assert_eq!(a.children[0].session_id, "b");
        assert_eq!(a.children[0].depth, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archived_parent_promotes_children_to_roots() {
        let sessions = vec![info("a", 1), info("b", 1)];
        let dir = std::env::temp_dir().join(format!("tg-tree2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = TangentsState::load(&dir);
        st.record_fork("b", "a");
        st.meta_mut("a").archived = true;
        let forest = build_forest(&sessions, &st);
        // a is hidden; b becomes a root.
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].session_id, "b");
        std::fs::remove_dir_all(&dir).ok();
    }
}
