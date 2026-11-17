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
    /// The folders inside `path` (the engine's home folder when absent), for picking a folder
    /// on the machine the engine runs on, which a client on another machine can't browse.
    ListDirectories {
        id: u64,
        path: Option<String>,
    },
    /// File operations on the engine's machine, for a client that can't touch its disk: an
    /// empty file or a folder, a move or rename, a copy (files and folders), and a move to the
    /// freedesktop Trash. Paths are absolute; nothing is overwritten.
    CreateEntry {
        id: u64,
        path: String,
        is_dir: bool,
    },
    MoveEntry {
        id: u64,
        from: String,
        to: String,
    },
    CopyEntry {
        id: u64,
        from: String,
        to: String,
    },
    TrashEntry {
        id: u64,
        path: String,
    },
    /// Runs `git` with `args` in `cwd` on the engine's machine: the status, branch and staging
    /// the client shows for a project it can't run git on itself.
    Git {
        id: u64,
        cwd: String,
        args: Vec<String>,
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
            | Request::SetUserSettings { id, .. }
            | Request::ListDirectories { id, .. }
            | Request::CreateEntry { id, .. }
            | Request::MoveEntry { id, .. }
            | Request::CopyEntry { id, .. }
            | Request::TrashEntry { id, .. }
            | Request::Git { id, .. } => *id,
        }
    }
}
