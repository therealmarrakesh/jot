//! Clean-room keystrokes, modifiers, key contexts, and typed `Command` keymap lookup.

use std::collections::HashMap;

pub(crate) use crate::command::Command;

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

/// Single keybinding mapping a keystroke in an optional context to a typed `Command`.
#[derive(Clone, Debug)]
pub(crate) struct KeyBinding {
    keystroke: Keystroke,
    command: Command,
    context: Option<String>,
}

impl KeyBinding {
    pub(crate) fn new(keystroke: &str, command: Command, context: Option<&str>) -> Self {
        Self {
            keystroke: Keystroke::parse(keystroke),
            command,
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

/// Registry of keybindings evaluated from deepest matching context / most recently added down.
#[derive(Default)]
pub(crate) struct Keymap {
    bindings: Vec<KeyBinding>,
    by_command: HashMap<Command, Vec<usize>>,
}

impl Keymap {
    pub(crate) fn add_bindings(&mut self, bindings: impl IntoIterator<Item = KeyBinding>) {
        for binding in bindings {
            let idx = self.bindings.len();
            self.by_command.entry(binding.command).or_default().push(idx);
            self.bindings.push(binding);
        }
    }

    /// Returns matching commands in priority order (deepest matching context / last added first).
    pub(crate) fn match_keystroke(
        &self,
        keystroke: &Keystroke,
        contexts: &[KeyContext],
    ) -> Vec<Command> {
        let mut matched: Vec<(usize, usize, Command)> = Vec::new();
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
            matched.push((depth, idx, binding.command));
        }
        matched.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        matched.into_iter().map(|(_, _, cmd)| cmd).collect()
    }

    pub(crate) fn shortcut_for(
        &self,
        command: Command,
        contexts: &[KeyContext],
    ) -> Option<KeyBinding> {
        let indices = self.by_command.get(&command)?;
        let mut best: Option<(usize, usize, &KeyBinding)> = None;
        for &idx in indices {
            let binding = &self.bindings[idx];
            if !binding.matches_context(contexts) {
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
    fn keymap_prefers_deeper_context_and_finds_shortcut_for_command() {
        let mut map = Keymap::default();
        map.add_bindings([
            KeyBinding::new("ctrl-s", Command::Save, Some("Jot")),
            KeyBinding::new("enter", Command::Enter, Some("Input")),
            KeyBinding::new("enter", Command::FindNext, Some("JotFindQuery")),
        ]);

        let contexts = [
            KeyContext::parse("Jot"),
            KeyContext::parse("Input"),
            KeyContext::parse("JotFindQuery"),
        ];
        let matched = map.match_keystroke(&Keystroke::parse("enter"), &contexts);
        assert_eq!(matched, vec![Command::FindNext, Command::Enter]);

        let shortcut = map
            .shortcut_for(Command::Save, &contexts)
            .expect("binding should resolve");
        assert_eq!(shortcut.keystroke().key, "s");
        assert!(shortcut.keystroke().modifiers.control);
    }
}
