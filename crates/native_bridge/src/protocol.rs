use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    OpenFolder {
        id: u64,
        path: String,
    },
    ListEntries {
        id: u64,
        worktree_id: u64,
    },
    OpenBuffer {
        id: u64,
        worktree_id: u64,
        path: String,
    },
    BufferText {
        id: u64,
        buffer_id: u64,
    },
    Highlights {
        id: u64,
        buffer_id: u64,
        start_row: u32,
        end_row: u32,
    },
    Edit {
        id: u64,
        buffer_id: u64,
        start_offset: usize,
        end_offset: usize,
        text: String,
    },
    Save {
        id: u64,
        buffer_id: u64,
    },
    Completions {
        id: u64,
        buffer_id: u64,
        offset: usize,
        /// The character typed that asked for completions ("." in `foo.`), if any.
        trigger: Option<String>,
    },
    Definition {
        id: u64,
        buffer_id: u64,
        offset: usize,
    },
    Rename {
        id: u64,
        buffer_id: u64,
        offset: usize,
        new_name: String,
    },
    Format {
        id: u64,
        buffer_id: u64,
    },
    Hover {
        id: u64,
        buffer_id: u64,
        offset: usize,
    },
    Diagnostics {
        id: u64,
        buffer_id: u64,
    },
    /// The JSON schema of Zed's user settings, the parsed defaults, and the bundled themes.
    SettingsSchema {
        id: u64,
    },
    /// Replaces the user settings layer (a Zed `settings.json`, comments allowed) and re-picks
    /// the syntax theme for `appearance` ("light" or "dark").
    SetUserSettings {
        id: u64,
        content: String,
        appearance: Option<String>,
    },
}

impl Request {
    pub fn id(&self) -> u64 {
        match self {
            Request::OpenFolder { id, .. }
            | Request::ListEntries { id, .. }
            | Request::OpenBuffer { id, .. }
            | Request::BufferText { id, .. }
            | Request::Highlights { id, .. }
            | Request::Edit { id, .. }
            | Request::Save { id, .. }
            | Request::Completions { id, .. }
            | Request::Definition { id, .. }
            | Request::Rename { id, .. }
            | Request::Format { id, .. }
            | Request::Hover { id, .. }
            | Request::Diagnostics { id, .. }
            | Request::SettingsSchema { id, .. }
            | Request::SetUserSettings { id, .. } => *id,
        }
    }
}
