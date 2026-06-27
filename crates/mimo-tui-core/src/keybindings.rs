#[derive(Debug, Clone, Copy)]
pub struct KeybindingInfo {
    pub chord: &'static str,
    pub description: &'static str,
}

pub const KEYBINDINGS: &[KeybindingInfo] = &[
    KeybindingInfo {
        chord: "Enter",
        description: "Send the draft, execute a slash command, or approve a pending tool request.",
    },
    KeybindingInfo {
        chord: "Tab",
        description: "Complete /commands or attach the current @path token.",
    },
    KeybindingInfo {
        chord: "Ctrl+J / Shift+Enter",
        description: "Insert a newline in the draft.",
    },
    KeybindingInfo {
        chord: "Ctrl+U",
        description: "Clear the current draft and keep it in history.",
    },
    KeybindingInfo {
        chord: "Ctrl+S",
        description: "Stash the current draft for later reuse.",
    },
    KeybindingInfo {
        chord: "Alt+R",
        description: "Browse cleared drafts and recent prompt history.",
    },
    KeybindingInfo {
        chord: "Ctrl+K",
        description: "Open the command palette.",
    },
    KeybindingInfo {
        chord: "Ctrl+R",
        description: "Open the saved-session picker.",
    },
    KeybindingInfo {
        chord: "Ctrl+L",
        description: "Open a pager for the latest message.",
    },
    KeybindingInfo {
        chord: "Ctrl+B",
        description: "Select a message to create a conversation branch.",
    },
    KeybindingInfo {
        chord: "F2 / Ctrl+Tab",
        description: "Cycle between plan, agent, and yolo modes.",
    },
    KeybindingInfo {
        chord: "@path + Tab",
        description: "Attach a file or directory from the workspace.",
    },
    KeybindingInfo {
        chord: "Backspace on empty draft",
        description: "Remove the most recently attached file or directory.",
    },
    KeybindingInfo {
        chord: "Left / Right",
        description: "Move the cursor inside the draft.",
    },
    KeybindingInfo {
        chord: "Home / End",
        description: "Jump to the start or end of the current line.",
    },
    KeybindingInfo {
        chord: "Ctrl+A / Ctrl+E",
        description: "Jump to the start or end of the current line.",
    },
    KeybindingInfo {
        chord: "Delete / Backspace",
        description: "Delete text around the cursor.",
    },
    KeybindingInfo {
        chord: "Up / Down",
        description: "Scroll the conversation or move in open menus.",
    },
    KeybindingInfo {
        chord: "PageUp / PageDown",
        description: "Scroll by one page.",
    },
    KeybindingInfo {
        chord: "Ctrl+Home / Ctrl+End",
        description: "Jump to the top or bottom of the conversation.",
    },
    KeybindingInfo {
        chord: "F1 / ?",
        description: "Open the searchable help overlay.",
    },
    KeybindingInfo {
        chord: "Esc / Ctrl+C",
        description: "Close overlays, cancel a generation, or quit.",
    },
    KeybindingInfo {
        chord: "y / n / a / r / p",
        description: "Approve, deny, or change approval mode in the tool prompt.",
    },
];
