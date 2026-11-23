//! Clean-room actions, keystrokes, modifiers, key contexts, and keymap lookup.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

/// Dynamic action dispatched through focus/element handlers.
pub(crate) trait Action: Any + 'static {
    fn boxed_clone(&self) -> Box<dyn Action>;
    fn partial_eq(&self, other: &dyn Action) -> bool;
    fn name(&self) -> &'static str;
}

impl dyn Action {
    #[inline]
    pub(crate) fn as_any(&self) -> &dyn Any {
        self
    }

    #[inline]
    pub(crate) fn downcast_ref<A: Action>(&self) -> Option<&A> {
        self.as_any().downcast_ref::<A>()
    }
}

impl Clone for Box<dyn Action> {
    fn clone(&self) -> Self {
        self.boxed_clone()
    }
}

macro_rules! impl_action {
    ($namespace:ident, $name:ident) => {
        impl $crate::ui::Action for $name {
            fn boxed_clone(&self) -> Box<dyn $crate::ui::Action> {
                Box::new(self.clone())
            }

            fn partial_eq(&self, other: &dyn $crate::ui::Action) -> bool {
                other
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|other| self == other)
            }

            fn name(&self) -> &'static str {
                concat!(stringify!($namespace), "::", stringify!($name))
            }
        }
    };
}

macro_rules! actions {
    ($namespace:ident, [$($name:ident),* $(,)?]) => {
        $(
            #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
            pub(crate) struct $name;
            $crate::ui::impl_action!($namespace, $name);
        )*
    };
}

pub(crate) use {actions, impl_action};

/// Keyboard modifier state.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Modifiers {
    pub(crate) control: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) platform: bool,
    pub(crate) function: bool,
}

impl Modifiers {
    #[inline]
    pub(crate) const fn control() -> Self {
        Self {
            control: true,
            alt: false,
            shift: false,
            platform: false,
            function: false,
        }
    }

    #[inline]
    pub(crate) fn modified(&self) -> bool {
        self.control || self.alt || self.shift || self.platform || self.function
    }
}

/// Normalized keyboard event keystroke.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Keystroke {
    pub(crate) modifiers: Modifiers,
    pub(crate) key: String,
    pub(crate) key_char: Option<String>,
}

impl Keystroke {
    pub(crate) fn parse(spec: &str) -> Self {
        let mut modifiers = Modifiers::default();
        let mut parts = spec.split('-').peekable();
        let mut key = String::new();

        while let Some(part) = parts.next() {
            // Handle trailing `-` key (e.g. `ctrl--` produces `["ctrl", "", ""]`).
            if part.is_empty() && parts.peek().is_some() {
                key = "-".to_string();
                break;
            }
            if parts.peek().is_some() {
                match part {
                    "ctrl" | "control" | "secondary" => modifiers.control = true,
                    "alt" => modifiers.alt = true,
                    "shift" => modifiers.shift = true,
                    "cmd" | "win" | "super" | "platform" => modifiers.platform = true,
                    "fn" => modifiers.function = true,
                    other => {
                        key = other.to_ascii_lowercase();
                        break;
                    }
                }
            } else {
                key = part.to_ascii_lowercase();
            }
        }
        if key.is_empty() && spec.ends_with('-') {
            key = "-".to_string();
        }
        Self {
            modifiers,
            key,
            key_char: None,
        }
    }

    pub(crate) fn matches(&self, incoming: &Keystroke) -> bool {
        if self.modifiers == incoming.modifiers && self.key == incoming.key {
            return true;
        }
        // Pressing Ctrl+Shift+= produces `+` on US keyboards; match `ctrl-+` cleanly.
        if self.key == "+"
            && (incoming.key == "+" || incoming.key == "=")
            && incoming.modifiers.shift
            && !self.modifiers.shift
            && self.modifiers.control == incoming.modifiers.control
            && self.modifiers.alt == incoming.modifiers.alt
            && self.modifiers.platform == incoming.modifiers.platform
        {
            return true;
        }
        false
    }
}

/// Space-separated key context tags (e.g. `"Jot"`, `"Input"`, `"JotMenu"`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyContext {
    tokens: Vec<String>,
}

impl KeyContext {
    pub(crate) fn parse(spec: &str) -> Self {
        Self {
            tokens: spec.split_whitespace().map(str::to_owned).collect(),
        }
    }

    pub(crate) fn contains(&self, token: &str) -> bool {
        self.tokens.iter().any(|t| t == token)
    }
}

/// Single keybinding mapping a keystroke in an optional context to an `Action`.
#[derive(Clone)]
pub(crate) struct KeyBinding {
    keystroke: Keystroke,
    action: Box<dyn Action>,
    context: Option<String>,
}

impl fmt::Debug for KeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyBinding")
            .field("keystroke", &self.keystroke)
            .field("action", &self.action.name())
            .field("context", &self.context)
            .finish()
    }
}

impl KeyBinding {
    pub(crate) fn new<A: Action>(
        keystroke: &str,
        action: A,
        context: Option<&str>,
    ) -> Self {
        Self {
            keystroke: Keystroke::parse(keystroke),
            action: Box::new(action),
            context: context.map(str::to_owned),
        }
    }

    #[inline]
    pub(crate) fn keystroke(&self) -> &Keystroke {
        &self.keystroke
    }

    fn matches_context(&self, contexts: &[KeyContext]) -> bool {
        match &self.context {
            None => true,
            Some(required) => contexts.iter().any(|ctx| ctx.contains(required)),
        }
    }
}

/// Registry of keybindings evaluated from highest precedence (most recently added) down.
#[derive(Default)]
pub(crate) struct Keymap {
    bindings: Vec<KeyBinding>,
    by_action: HashMap<TypeId, Vec<usize>>,
}

impl Keymap {
    pub(crate) fn add_bindings(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        for binding in bindings {
            let idx = self.bindings.len();
            let type_id = binding.action.as_any().type_id();
            self.by_action.entry(type_id).or_default().push(idx);
            self.bindings.push(binding);
        }
    }

    /// Returns matching bindings in priority order (deepest matching context / last added first).
    pub(crate) fn match_keystroke(
        &self,
        keystroke: &Keystroke,
        contexts: &[KeyContext],
    ) -> Vec<Box<dyn Action>> {
        let mut matched: Vec<(usize, usize, Box<dyn Action>)> = Vec::new();
        for (idx, binding) in self.bindings.iter().enumerate() {
            if !binding.keystroke.matches(keystroke) {
                continue;
            }
            let depth = match &binding.context {
                None => 0,
                Some(req) => match contexts.iter().rposition(|ctx| ctx.contains(req)) {
                    Some(pos) => pos + 1,
                    None => continue,
                },
            };
            matched.push((depth, idx, binding.action.boxed_clone()));
        }
        matched.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        matched.into_iter().map(|(_, _, action)| action).collect()
    }

    pub(crate) fn highest_precedence_for_action(
        &self,
        action: &dyn Action,
        contexts: &[KeyContext],
    ) -> Option<KeyBinding> {
        let type_id = action.as_any().type_id();
        let indices = self.by_action.get(&type_id)?;
        let mut best: Option<(usize, usize, &KeyBinding)> = None;
        for &idx in indices {
            let binding = &self.bindings[idx];
            if !binding.action.partial_eq(action) || !binding.matches_context(contexts) {
                continue;
            }
            let depth = match &binding.context {
                None => 0,
                Some(req) => contexts
                    .iter()
                    .rposition(|ctx| ctx.contains(req))
                    .map_or(0, |p| p + 1),
            };
            let take = match best {
                None => true,
                Some((best_depth, best_idx, _)) => {
                    depth > best_depth || (depth == best_depth && idx < best_idx)
                }
            };
            if take {
                best = Some((depth, idx, binding));
            }
        }
        best.map(|(_, _, b)| b.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_hyphen_key() {
        let k = Keystroke::parse("ctrl--");
        assert!(k.modifiers.control);
        assert_eq!(k.key, "-");

        let s = Keystroke::parse("ctrl-shift-s");
        assert!(s.modifiers.control && s.modifiers.shift && !s.modifiers.alt);
        assert_eq!(s.key, "s");

        let sec = Keystroke::parse("secondary-enter");
        assert!(sec.modifiers.control);
        assert_eq!(sec.key, "enter");
    }

    #[test]
    fn keymap_prefers_deeper_context_and_finds_shortcut_for_action() {
        #[derive(Clone, PartialEq, Eq)]
        struct TestSave;
        impl Action for TestSave {
            fn boxed_clone(&self) -> Box<dyn Action> {
                Box::new(self.clone())
            }
            fn partial_eq(&self, other: &dyn Action) -> bool {
                other.as_any().is::<Self>()
            }
            fn name(&self) -> &'static str {
                "test::TestSave"
            }
        }

        let mut map = Keymap::default();
        map.add_bindings([KeyBinding::new("ctrl-s", TestSave, Some("Jot"))]);

        let contexts = [KeyContext::parse("Jot"), KeyContext::parse("Input")];
        let matched = map.match_keystroke(&Keystroke::parse("ctrl-s"), &contexts);
        assert_eq!(matched.len(), 1);

        let shortcut = map
            .highest_precedence_for_action(&TestSave, &contexts)
            .expect("binding should resolve");
        assert_eq!(shortcut.keystroke().key, "s");
        assert!(shortcut.keystroke().modifiers.control);
    }
}
