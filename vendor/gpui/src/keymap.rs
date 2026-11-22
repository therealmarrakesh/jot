use crate::collections::TypeIdHashMap;
use crate::smallvec::SmallVec;
use crate::{Action, Keystroke, SharedString};

/// A named scope attached to an element in the dispatch tree (e.g. `"Jot"`, `"Input"`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyContext(pub SharedString);

impl From<&'static str> for KeyContext {
    fn from(value: &'static str) -> Self {
        Self(SharedString::new_static(value))
    }
}

impl From<String> for KeyContext {
    fn from(value: String) -> Self {
        Self(SharedString::from(value))
    }
}

impl From<SharedString> for KeyContext {
    fn from(value: SharedString) -> Self {
        Self(value)
    }
}

/// A single-keystroke binding to an action, optionally scoped to a named context.
pub struct KeyBinding {
    pub(crate) action: Box<dyn Action>,
    pub(crate) keystroke: Keystroke,
    pub(crate) context: Option<SharedString>,
}

impl Clone for KeyBinding {
    fn clone(&self) -> Self {
        Self {
            action: self.action.boxed_clone(),
            keystroke: self.keystroke.clone(),
            context: self.context.clone(),
        }
    }
}

impl KeyBinding {
    /// Construct a new single-keystroke binding. Panics if `keystroke` cannot be parsed.
    pub fn new<A: Action>(keystroke: &str, action: A, context: Option<&str>) -> Self {
        Self {
            action: Box::new(action),
            keystroke: Keystroke::parse(keystroke).unwrap(),
            context: context.map(SharedString::new),
        }
    }

    /// Check if `typed` matches this binding's keystroke.
    pub fn match_keystroke(&self, typed: &Keystroke) -> bool {
        typed.should_match(&self.keystroke)
    }

    /// Get the keystroke associated with this binding.
    pub fn keystroke(&self) -> &Keystroke {
        &self.keystroke
    }

    /// Get the action associated with this binding.
    pub fn action(&self) -> &dyn Action {
        self.action.as_ref()
    }
}

impl std::fmt::Debug for KeyBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyBinding")
            .field("keystroke", &self.keystroke)
            .field("context", &self.context)
            .field("action", &self.action.name())
            .finish()
    }
}

/// A collection of single-keystroke bindings for the application.
#[derive(Default)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
    binding_indices_by_action_id: TypeIdHashMap<SmallVec<[usize; 3]>>,
}

impl Keymap {
    /// Add bindings to the keymap.
    pub fn add_bindings<T: IntoIterator<Item = KeyBinding>>(&mut self, bindings: T) {
        for binding in bindings {
            let action_id = binding.action().as_any().type_id();
            self.binding_indices_by_action_id
                .entry(action_id)
                .or_default()
                .push(self.bindings.len());
            self.bindings.push(binding);
        }
    }

    /// Clear all bindings from the keymap.
    pub fn clear(&mut self) {
        self.bindings.clear();
        self.binding_indices_by_action_id.clear();
    }

    /// Iterate over all bindings for the given action in registration order.
    pub fn bindings_for_action<'a>(
        &'a self,
        action: &'a dyn Action,
    ) -> impl 'a + DoubleEndedIterator<Item = &'a KeyBinding> {
        let action_id = action.type_id();
        self.binding_indices_by_action_id
            .get(&action_id)
            .map_or(&[] as _, SmallVec::as_slice)
            .iter()
            .filter_map(|ix| {
                let binding = &self.bindings[*ix];
                binding.action().partial_eq(action).then_some(binding)
            })
    }

    /// Returns matching bindings for `input` in `context_stack`, ordered by deepest context
    /// first and later registration order second.
    pub fn bindings_for_input(
        &self,
        input: &Keystroke,
        context_stack: &[KeyContext],
    ) -> SmallVec<[KeyBinding; 1]> {
        let mut matched = SmallVec::<[(usize, usize, &KeyBinding); 1]>::new();

        for (ix, binding) in self.bindings.iter().enumerate() {
            let Some(depth) = Self::binding_depth(binding, context_stack) else {
                continue;
            };
            if binding.match_keystroke(input) {
                matched.push((depth, ix, binding));
            }
        }

        matched.sort_by(|(depth_a, ix_a, _), (depth_b, ix_b, _)| {
            depth_b.cmp(depth_a).then(ix_b.cmp(ix_a))
        });

        matched
            .into_iter()
            .map(|(_, _, binding)| binding.clone())
            .collect()
    }

    fn binding_depth(binding: &KeyBinding, contexts: &[KeyContext]) -> Option<usize> {
        match &binding.context {
            Some(ctx) => contexts.iter().rposition(|c| c.0 == *ctx).map(|i| i + 1),
            None => Some(contexts.len()),
        }
    }
}
