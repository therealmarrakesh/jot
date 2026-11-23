use crate::ui::collections::FxHashMap;
use crate::ui::smallvec::SmallVec;
use crate::ui::{
    Action, App, DispatchPhase, FocusId, KeyBinding, KeyContext, Keymap, Keystroke,
    ModifiersChangedEvent, Window,
};
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    rc::Rc,
};

/// ID of a node within `DispatchTree`. Not stable between frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct DispatchNodeId(usize);

pub(crate) struct DispatchTree {
    node_stack: Vec<DispatchNodeId>,
    pub(crate) context_stack: Vec<KeyContext>,
    nodes: Vec<DispatchNode>,
    focusable_node_ids: FxHashMap<FocusId, DispatchNodeId>,
    keymap: Rc<RefCell<Keymap>>,
}

#[derive(Default)]
pub(crate) struct DispatchNode {
    pub(crate) key_listeners: Vec<KeyListener>,
    pub(crate) action_listeners: Vec<DispatchActionListener>,
    pub(crate) modifiers_changed_listeners: Vec<ModifiersChangedListener>,
    pub(crate) context: Option<KeyContext>,
    pub(crate) focus_id: Option<FocusId>,
    parent: Option<DispatchNodeId>,
}

type KeyListener = Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App)>;
type ModifiersChangedListener = Rc<dyn Fn(&ModifiersChangedEvent, &mut Window, &mut App)>;

type ActionListener = Rc<dyn Fn(&dyn Any, DispatchPhase, &mut Window, &mut App)>;

#[derive(Clone)]
pub(crate) struct DispatchActionListener {
    pub(crate) action_type: TypeId,
    pub(crate) listener: ActionListener,
}

impl DispatchTree {
    pub(crate) fn new(keymap: Rc<RefCell<Keymap>>) -> Self {
        Self {
            node_stack: Vec::new(),
            context_stack: Vec::new(),
            nodes: Vec::new(),
            focusable_node_ids: FxHashMap::default(),
            keymap,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.node_stack.clear();
        self.context_stack.clear();
        self.nodes.clear();
        self.focusable_node_ids.clear();
    }

    pub(crate) fn push_node(&mut self) -> DispatchNodeId {
        let parent = self.node_stack.last().copied();
        let node_id = DispatchNodeId(self.nodes.len());

        self.nodes.push(DispatchNode {
            parent,
            ..Default::default()
        });
        self.node_stack.push(node_id);
        node_id
    }

    pub(crate) fn set_active_node(&mut self, node_id: DispatchNodeId) {
        let next_node_parent = self.nodes[node_id.0].parent;
        while self.node_stack.last().copied() != next_node_parent && !self.node_stack.is_empty() {
            self.pop_node();
        }

        if self.node_stack.last().copied() == next_node_parent {
            self.node_stack.push(node_id);
            let active_node = &self.nodes[node_id.0];
            if let Some(context) = active_node.context.clone() {
                self.context_stack.push(context);
            }
        } else {
            debug_assert_eq!(self.node_stack.len(), 0);

            let mut current_node_id = Some(node_id);
            while let Some(node_id) = current_node_id {
                let node = &self.nodes[node_id.0];
                if let Some(context) = node.context.clone() {
                    self.context_stack.push(context);
                }
                self.node_stack.push(node_id);
                current_node_id = node.parent;
            }

            self.context_stack.reverse();
            self.node_stack.reverse();
        }
    }

    pub(crate) fn set_key_context(&mut self, context: KeyContext) {
        self.active_node().context = Some(context.clone());
        self.context_stack.push(context);
    }

    pub(crate) fn set_focus_id(&mut self, focus_id: FocusId) {
        let node_id = *self.node_stack.last().unwrap();
        self.nodes[node_id.0].focus_id = Some(focus_id);
        self.focusable_node_ids.insert(focus_id, node_id);
    }

    pub(crate) fn pop_node(&mut self) {
        let node = &self.nodes[self.active_node_id().unwrap().0];
        if node.context.is_some() {
            self.context_stack.pop();
        }
        self.node_stack.pop();
    }

    pub(crate) fn on_key_event(&mut self, listener: KeyListener) {
        self.active_node().key_listeners.push(listener);
    }

    pub(crate) fn on_modifiers_changed(&mut self, listener: ModifiersChangedListener) {
        self.active_node()
            .modifiers_changed_listeners
            .push(listener);
    }

    pub(crate) fn on_action(&mut self, action_type: TypeId, listener: ActionListener) {
        self.active_node()
            .action_listeners
            .push(DispatchActionListener {
                action_type,
                listener,
            });
    }

    pub(crate) fn focus_contains(&self, parent: FocusId, child: FocusId) -> bool {
        if parent == child {
            return true;
        }

        if let Some(parent_node_id) = self.focusable_node_ids.get(&parent) {
            let mut current_node_id = self.focusable_node_ids.get(&child).copied();
            while let Some(node_id) = current_node_id {
                if node_id == *parent_node_id {
                    return true;
                }
                current_node_id = self.nodes[node_id.0].parent;
            }
        }
        false
    }

    pub(crate) fn highest_precedence_binding_for_action(
        &self,
        action: &dyn Action,
        context_stack: &[KeyContext],
    ) -> Option<KeyBinding> {
        let keymap = self.keymap.borrow();
        keymap
            .bindings_for_action(action)
            .rev()
            .find(|binding| Self::binding_not_shadowed(&keymap, binding, context_stack))
            .cloned()
    }

    fn binding_not_shadowed(
        keymap: &Keymap,
        binding: &KeyBinding,
        context_stack: &[KeyContext],
    ) -> bool {
        let bindings = keymap.bindings_for_input(&binding.keystroke, context_stack);
        bindings
            .first()
            .is_some_and(|found| found.action.partial_eq(binding.action.as_ref()))
    }

    pub(crate) fn dispatch_key(
        &self,
        keystroke: &Keystroke,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
    ) -> (SmallVec<[KeyBinding; 1]>, Vec<KeyContext>) {
        let context_stack: Vec<KeyContext> = dispatch_path
            .iter()
            .filter_map(|node_id| self.node(*node_id).context.clone())
            .collect();
        let bindings = self
            .keymap
            .borrow()
            .bindings_for_input(keystroke, &context_stack);
        (bindings, context_stack)
    }

    pub(crate) fn dispatch_path(&self, target: DispatchNodeId) -> SmallVec<[DispatchNodeId; 32]> {
        let mut dispatch_path: SmallVec<[DispatchNodeId; 32]> = SmallVec::new();
        let mut current_node_id = Some(target);
        while let Some(node_id) = current_node_id {
            dispatch_path.push(node_id);
            current_node_id = self.nodes.get(node_id.0).and_then(|node| node.parent);
        }
        dispatch_path.reverse();
        dispatch_path
    }

    pub(crate) fn focus_path(&self, focus_id: FocusId) -> SmallVec<[FocusId; 8]> {
        let mut focus_path: SmallVec<[FocusId; 8]> = SmallVec::new();
        let mut current_node_id = self.focusable_node_ids.get(&focus_id).copied();
        while let Some(node_id) = current_node_id {
            let node = self.node(node_id);
            if let Some(focus_id) = node.focus_id {
                focus_path.push(focus_id);
            }
            current_node_id = node.parent;
        }
        focus_path.reverse();
        focus_path
    }

    pub(crate) fn node(&self, node_id: DispatchNodeId) -> &DispatchNode {
        &self.nodes[node_id.0]
    }

    fn active_node(&mut self) -> &mut DispatchNode {
        let active_node_id = self.active_node_id().unwrap();
        &mut self.nodes[active_node_id.0]
    }

    pub(crate) fn focusable_node_id(&self, target: FocusId) -> Option<DispatchNodeId> {
        self.focusable_node_ids.get(&target).copied()
    }

    pub(crate) fn root_node_id(&self) -> DispatchNodeId {
        debug_assert!(!self.nodes.is_empty());
        DispatchNodeId(0)
    }

    pub(crate) fn active_node_id(&self) -> Option<DispatchNodeId> {
        self.node_stack.last().copied()
    }
}
