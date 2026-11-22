use std::collections::BTreeSet;
use std::fmt::Debug;
use std::ops::Bound;

use crate::collections::FxHashMap;
use crate::{FocusHandle, FocusId};

/// Represents a collection of focus handles using the tab-index APIs.
#[derive(Debug, Default)]
pub(crate) struct TabStopMap {
    current_path: TabStopPath,
    pub(crate) insertion_history: Vec<TabStopOperation>,
    by_id: FxHashMap<FocusId, TabStopNode>,
    order: BTreeSet<TabStopNode>,
}

#[derive(Debug, Clone)]
pub enum TabStopOperation {
    Insert(FocusHandle),
    Group(TabIndex),
    GroupEnd,
}

impl TabStopOperation {
    fn focus_handle(&self) -> Option<&FocusHandle> {
        match self {
            TabStopOperation::Insert(focus_handle) => Some(focus_handle),
            _ => None,
        }
    }
}

type TabIndex = isize;

#[derive(Debug, Default, PartialEq, Eq, Clone, Ord, PartialOrd)]
struct TabStopPath(crate::smallvec::SmallVec<[TabIndex; 6]>);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TabStopNode {
    path: TabStopPath,
    node_insertion_index: usize,
    tab_stop: bool,
}

impl Ord for TabStopNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path
            .cmp(&other.path)
            .then(self.node_insertion_index.cmp(&other.node_insertion_index))
    }
}

impl PartialOrd for TabStopNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl TabStopMap {
    pub fn insert(&mut self, focus_handle: &FocusHandle) {
        self.insertion_history
            .push(TabStopOperation::Insert(focus_handle.clone()));
        let mut path = self.current_path.clone();
        path.0.push(focus_handle.tab_index);
        let order = TabStopNode {
            node_insertion_index: self.insertion_history.len() - 1,
            tab_stop: focus_handle.tab_stop,
            path,
        };
        if let Some(prev) = self.by_id.insert(focus_handle.id, order.clone()) {
            self.order.remove(&prev);
        }
        self.order.insert(order);
    }

    pub fn begin_group(&mut self, tab_index: isize) {
        self.insertion_history
            .push(TabStopOperation::Group(tab_index));
        self.current_path.0.push(tab_index);
    }

    pub fn end_group(&mut self) {
        self.insertion_history.push(TabStopOperation::GroupEnd);
        self.current_path.0.pop();
    }

    pub fn clear(&mut self) {
        self.current_path.0.clear();
        self.insertion_history.clear();
        self.by_id.clear();
        self.order.clear();
    }

    pub fn next(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        let Some(focused_id) = focused_id else {
            let first = self.order.iter().find(|node| node.tab_stop)?;
            return self.focus_handle_for_order(first);
        };

        let Some(node) = self.tab_node_for_focus_id(focused_id) else {
            return self.next(None);
        };
        if let Some(item) = self.next_inner(node) {
            self.focus_handle_for_order(item)
        } else {
            self.next(None)
        }
    }

    fn next_inner(&self, node: &TabStopNode) -> Option<&TabStopNode> {
        self.order
            .range((Bound::Excluded(node), Bound::Unbounded))
            .find(|item| item.tab_stop)
    }

    pub fn prev(&self, focused_id: Option<&FocusId>) -> Option<FocusHandle> {
        let Some(focused_id) = focused_id else {
            let last = self.order.iter().rev().find(|node| node.tab_stop)?;
            return self.focus_handle_for_order(last);
        };

        let Some(node) = self.tab_node_for_focus_id(focused_id) else {
            return self.prev(None);
        };
        if let Some(item) = self.prev_inner(node) {
            self.focus_handle_for_order(item)
        } else {
            self.prev(None)
        }
    }

    fn prev_inner(&self, node: &TabStopNode) -> Option<&TabStopNode> {
        self.order
            .range((Bound::Unbounded, Bound::Excluded(node)))
            .rev()
            .find(|item| item.tab_stop)
    }

    pub fn replay(&mut self, nodes: &[TabStopOperation]) {
        for node in nodes {
            match node {
                TabStopOperation::Insert(focus_handle) => self.insert(focus_handle),
                TabStopOperation::Group(tab_index) => self.begin_group(*tab_index),
                TabStopOperation::GroupEnd => self.end_group(),
            }
        }
    }

    pub fn paint_index(&self) -> usize {
        self.insertion_history.len()
    }

    pub(crate) fn tab_stop_count(&self) -> usize {
        self.by_id.values().filter(|node| node.tab_stop).count()
    }

    fn focus_handle_for_order(&self, order: &TabStopNode) -> Option<FocusHandle> {
        let handle = self.insertion_history[order.node_insertion_index].focus_handle();
        debug_assert!(
            handle.is_some(),
            "The order node did not correspond to an element, this is a GPUI bug"
        );
        handle.cloned()
    }

    fn tab_node_for_focus_id(&self, focused_id: &FocusId) -> Option<&TabStopNode> {
        self.by_id.get(focused_id)
    }
}
