use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context as _, Result};
use futures::FutureExt as _;
use collections::{HashMap as CollectionsHashMap, HashSet as CollectionsHashSet};
use client::{Client, UserStore};
use fs::{Fs, RealFs};
use gpui::{App, AppContext as _, AsyncApp, Entity};
use http_client::HttpClientWithUrl;
use language::{Buffer, LanguageAwareStyling, LanguageRegistry, ToOffset as _};
use node_runtime::{NodeBinaryOptions, NodeRuntime};
use project::lsp_store::{FormatTrigger, LspFormatTarget, OpenLspBufferHandle};
use project::{LocalProjectFlags, Project};
use reqwest_client::ReqwestClient;
use serde_json::{json, Value};
use settings::{ParseStatus, SettingsJsonSchemaParams, SettingsStore};
use theme::{Appearance, Theme, ThemeRegistry};
use worktree::Worktree;

use crate::protocol::Request;

/// How long a completions request waits for the language servers.
const COMPLETIONS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1200);

#[derive(Clone)]
pub struct Bridge {
    project: Entity<Project>,
    languages: Arc<LanguageRegistry>,
    theme_registry: Arc<ThemeRegistry>,
    /// Swapped by `set_user_settings` when the `theme` setting or the app's appearance changes.
    theme: Arc<Mutex<Arc<Theme>>>,
    worktrees: Arc<Mutex<HashMap<u64, Entity<Worktree>>>>,
    // `BufferStore` only tracks buffers weakly (so they can be dropped once no editor is
    // showing them); we must hold our own strong reference for as long as a client might
    // still refer to this buffer_id. The LSP handle must likewise be held for as long as we
    // want that buffer registered with its language server.
    buffers: Arc<Mutex<HashMap<u64, (Entity<Buffer>, OpenLspBufferHandle)>>>,
}

impl Bridge {
    pub fn new(cx: &mut App) -> Self {
        settings::init(cx);
        release_channel::init(semver::Version::new(0, 1, 0), cx);
        gpui_tokio::init(cx);

        let fs: Arc<dyn Fs> = RealFs::new(None, cx.background_executor().clone());
        let mut languages = LanguageRegistry::new(cx.background_executor().clone());
        languages.set_language_server_download_dir(paths::languages_dir().clone());
        let languages = Arc::new(languages);
        // Node from the PATH (or Zed's own managed download when there is none), so the
        // npm-installed servers — TypeScript, JSON, CSS, YAML, Pyright, Tailwind — can run.
        let node = NodeRuntime::new(
            Arc::new(ReqwestClient::new()),
            None,
            watch::channel(Some(NodeBinaryOptions {
                allow_path_lookup: true,
                allow_binary_download: true,
                use_paths: None,
            }))
            .1,
        );
        languages::init(languages.clone(), fs.clone(), node.clone(), cx);
        crate::swift::register(&languages);

        let theme_registry = Arc::new(ThemeRegistry::new(Box::new(assets::Assets)));
        theme_settings::load_bundled_themes(&theme_registry);
        // The editor themes PitsiUI's themes are read from, which Zed doesn't bundle. Each is
        // its upstream Zed extension's JSON (MIT; licences beside them in `themes/`).
        for bytes in [
            include_bytes!("../themes/dracula.json").as_slice(),
            include_bytes!("../themes/rose-pine.json"),
            include_bytes!("../themes/rose-pine-dawn.json"),
            include_bytes!("../themes/nord.json"),
            include_bytes!("../themes/tokyo-night.json"),
        ] {
            util::ResultExt::log_err(theme_settings::load_user_theme(&theme_registry, bytes));
        }
        let theme = theme_registry
            .get("One Dark")
            .expect("the built-in \"One Dark\" theme is always registered");
        languages.set_theme(theme.clone());

        // Inert: `Client::new`/`UserStore::new` only build in-memory state. Nothing connects
        // or authenticates unless we call `authenticate_and_connect`, which we never do — we
        // only need these types to satisfy `Project::local`'s signature so we can reach its
        // public LSP wrappers (`register_buffer_with_language_servers`, `completions`, `hover`).
        let http_client = Arc::new(HttpClientWithUrl::new(
            Arc::new(ReqwestClient::new()),
            "http://localhost".to_string(),
            None,
        ));
        let client = Client::new(Arc::new(clock::RealSystemClock), http_client, cx);
        let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));

        // Passing our own environment as the "CLI environment" makes `ProjectEnvironment`
        // use it directly (see `get_cli_environment`) instead of spawning a login shell to
        // capture PATH/etc for every language server start — that shell-capture subprocess
        // hangs in this headless setup (likely because it inherits our own long-lived stdin
        // pipe), silently stalling every LSP launch with no error logged.
        let cli_environment = Some(std::env::vars().collect());

        let project = Project::local(
            client,
            node,
            user_store,
            languages.clone(),
            fs,
            cli_environment,
            LocalProjectFlags::default(),
            cx,
        );

        Self {
            project,
            languages,
            theme_registry,
            theme: Arc::new(Mutex::new(theme)),
            worktrees: Arc::new(Mutex::new(HashMap::new())),
            buffers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn handle(&self, request: Request, cx: &mut AsyncApp) -> Value {
        let id = request.id();
        match self.handle_inner(request, cx).await {
            Ok(ok) => json!({ "id": id, "ok": ok }),
            Err(error) => json!({ "id": id, "err": error.to_string() }),
        }
    }

    async fn handle_inner(&self, request: Request, cx: &mut AsyncApp) -> Result<Value> {
        match request {
            Request::OpenFolder { path, .. } => self.open_folder(path, cx).await,
            Request::ListEntries { worktree_id, .. } => self.list_entries(worktree_id, cx),
            Request::OpenBuffer {
                worktree_id, path, ..
            } => self.open_buffer(worktree_id, path, cx).await,
            Request::BufferText { buffer_id, .. } => self.buffer_text(buffer_id, cx),
            Request::Highlights {
                buffer_id,
                start_row,
                end_row,
                ..
            } => self.highlights(buffer_id, start_row, end_row, cx),
            Request::Edit {
                buffer_id,
                start_offset,
                end_offset,
                text,
                ..
            } => self.edit(buffer_id, start_offset, end_offset, text, cx),
            Request::Save { buffer_id, .. } => self.save(buffer_id, cx).await,
            Request::Completions {
                buffer_id,
                offset,
                trigger,
                ..
            } => self.completions(buffer_id, offset, trigger, cx).await,
            Request::Definition {
                buffer_id, offset, ..
            } => self.definition(buffer_id, offset, cx).await,
            Request::Rename {
                buffer_id,
                offset,
                new_name,
                ..
            } => self.rename(buffer_id, offset, new_name, cx).await,
            Request::Format { buffer_id, .. } => self.format(buffer_id, cx).await,
            Request::Hover {
                buffer_id, offset, ..
            } => self.hover(buffer_id, offset, cx).await,
            Request::Diagnostics { buffer_id, .. } => self.diagnostics(buffer_id, cx),
            Request::SettingsSchema { .. } => self.settings_schema(),
            Request::SetUserSettings {
                content,
                appearance,
                ..
            } => self.set_user_settings(content, appearance, cx),
        }
    }

    async fn open_folder(&self, path: String, cx: &mut AsyncApp) -> Result<Value> {
        let worktree = self
            .project
            .update(cx, |project, cx| project.create_worktree(path, true, cx))
            .await
            .map_err(|error| anyhow!(error))?;
        let scan_complete = worktree.read_with(cx, |worktree, _| {
            worktree
                .as_local()
                .expect("worktree is always local")
                .scan_complete()
        });
        scan_complete.await;
        let worktree_id = worktree.read_with(cx, |worktree, _| worktree.id());
        let worktree_id = usize::from(worktree_id) as u64;
        self.worktrees.lock().unwrap().insert(worktree_id, worktree);
        Ok(json!({ "worktree_id": worktree_id }))
    }

    fn list_entries(&self, worktree_id: u64, cx: &mut AsyncApp) -> Result<Value> {
        let worktree = self.worktree_for_id(worktree_id)?;
        let entries = worktree.read_with(cx, |worktree, _| {
            worktree
                .snapshot()
                .entries(false, 0)
                .map(|entry| {
                    json!({
                        "path": entry.path.as_unix_str(),
                        "is_dir": entry.is_dir(),
                        "size": entry.size,
                    })
                })
                .collect::<Vec<_>>()
        });
        Ok(json!({ "entries": entries }))
    }

    async fn open_buffer(
        &self,
        worktree_id: u64,
        path: String,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let worktree = self.worktree_for_id(worktree_id)?;
        let abs_path = worktree.read_with(cx, |worktree, _| worktree.abs_path().join(&path));
        let buffer = self
            .project
            .update(cx, |project, cx| project.open_local_buffer(abs_path, cx))
            .await?;
        self.assign_language(&buffer, cx).await?;
        let lsp_handle = self.project.update(cx, |project, cx| {
            project.register_buffer_with_language_servers(&buffer, cx)
        });
        let buffer_id = u64::from(buffer.read_with(cx, |buffer, _| buffer.remote_id()));
        self.buffers
            .lock()
            .unwrap()
            .insert(buffer_id, (buffer, lsp_handle));
        Ok(json!({ "buffer_id": buffer_id }))
    }

    async fn assign_language(&self, buffer: &Entity<Buffer>, cx: &mut AsyncApp) -> Result<()> {
        let file = buffer.read_with(cx, |buffer, _| buffer.file().cloned());
        let Some(file) = file else {
            return Ok(());
        };
        let content = buffer.read_with(cx, |buffer, _| buffer.as_rope().clone());
        let languages = self.project.read_with(cx, |project, _| project.languages().clone());
        let language_id =
            cx.update(|cx| languages.language_for_file(&file, Some(&content), cx));
        let Some(language_id) = language_id else {
            return Ok(());
        };
        let language = languages
            .load_language(language_id)
            .await
            .context("language registry dropped the load request")??;
        buffer.update(cx, |buffer, cx| {
            // Must be set before `set_language`: it reparses internally as soon as the
            // language is assigned, and without a sync_parse_timeout that reparse only
            // kicks off a background task, racing our very next `highlights` request
            // against a parse that hasn't finished yet.
            buffer.set_sync_parse_timeout(Some(std::time::Duration::from_secs(1)));
            buffer.set_language(Some(language), cx);
        });
        // A large file, or the first one in a language, can outlast that timeout; the first
        // `highlights` request would then colour nothing.
        let parsed = buffer.read_with(cx, |buffer, _| buffer.parsing_idle());
        parsed.await;
        Ok(())
    }

    fn buffer_text(&self, buffer_id: u64, cx: &mut AsyncApp) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let (text, language, editing) = buffer.read_with(cx, |buffer, _| {
            let editing = buffer.language().map(|language| {
                let config = language.config();
                // What the native editor needs to type the way Zed does: the comment prefix
                // for toggling, and which brackets close themselves and indent on Return.
                let line_comments = config
                    .line_comments
                    .iter()
                    .map(|prefix| prefix.trim_end().to_string())
                    .collect::<Vec<_>>();
                let brackets = config
                    .brackets
                    .pairs
                    .iter()
                    .map(|pair| {
                        json!({
                            "start": pair.start,
                            "end": pair.end,
                            "close": pair.close,
                            "newline": pair.newline,
                        })
                    })
                    .collect::<Vec<_>>();
                json!({
                    "line_comments": line_comments,
                    "brackets": brackets,
                    "autoclose_before": config.autoclose_before,
                })
            });
            (
                buffer.text(),
                buffer.language().map(|l| l.name().to_string()),
                editing,
            )
        });
        Ok(json!({ "text": text, "language": language, "editing": editing }))
    }

    fn highlights(
        &self,
        buffer_id: u64,
        start_row: u32,
        end_row: u32,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let theme = self.theme.lock().unwrap().clone();
        let runs = buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            let max_point = snapshot.max_point();
            let start_point = text::Point::new(start_row.min(max_point.row), 0);
            let end_point = if end_row >= max_point.row {
                max_point
            } else {
                text::Point::new(end_row, 0)
            };
            let start_offset = snapshot.point_to_offset(start_point);
            let end_offset = snapshot.point_to_offset(end_point);
            snapshot
                .chunks(
                    start_offset..end_offset,
                    LanguageAwareStyling {
                        tree_sitter: true,
                        diagnostics: false,
                    },
                )
                .map(|chunk| {
                    let color = chunk
                        .syntax_highlight_id
                        .and_then(|id| theme.syntax().get(id))
                        .and_then(|style| style.color)
                        .map(|color| hsla_to_hex(color));
                    json!({ "text": chunk.text, "color": color })
                })
                .collect::<Vec<_>>()
        });
        Ok(json!({ "runs": runs }))
    }

    fn edit(
        &self,
        buffer_id: u64,
        start_offset: usize,
        end_offset: usize,
        text: String,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(start_offset..end_offset, text)], None, cx);
        });
        Ok(json!({}))
    }

    async fn save(&self, buffer_id: u64, cx: &mut AsyncApp) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        // Zed formats on save in its editor layer, which this process doesn't run, so the
        // bridge does it here. `FormatTrigger::Save` honours `format_on_save`,
        // `remove_trailing_whitespace_on_save` and `ensure_final_newline_on_save`; a formatter
        // failure must not stop the save itself.
        let mut buffers = CollectionsHashSet::default();
        buffers.insert(buffer.clone());
        let format = self.project.update(cx, |project, cx| {
            project.format(buffers, LspFormatTarget::Buffers, true, FormatTrigger::Save, cx)
        });
        if let Err(error) = format.await {
            log::warn!("format on save failed: {error:#}");
        }
        self.project
            .update(cx, |project, cx| project.save_buffer(buffer, cx))
            .await?;
        Ok(json!({}))
    }

    async fn completions(
        &self,
        buffer_id: u64,
        offset: usize,
        trigger: Option<String>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let context = match trigger {
            Some(character) => lsp_types::CompletionContext {
                trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(character),
            },
            None => lsp_types::CompletionContext {
                trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
                trigger_character: None,
            },
        };
        let request = self.project.update(cx, |project, cx| {
            project.completions(&buffer, offset, context, cx)
        });
        // Requests are answered one at a time, so a language server that is still starting
        // (or never answers) would hold up every request after this one. Past the timeout the
        // app falls back to the buffer's words, as Zed's `words: "fallback"` does.
        let timeout = cx.background_executor().timer(COMPLETIONS_TIMEOUT);
        let responses = futures::select_biased! {
            responses = request.fuse() => responses?,
            _ = timeout.fuse() => return Ok(json!({ "completions": [], "timed_out": true })),
        };
        let items = buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            responses
                .into_iter()
                .flat_map(|response| response.completions)
                .map(|completion| {
                    let range = completion.replace_range.start.to_offset(&snapshot)
                        ..completion.replace_range.end.to_offset(&snapshot);
                    let lsp_completion = completion.source.lsp_completion(true);
                    let is_snippet = lsp_completion.as_ref().is_some_and(|item| {
                        item.insert_text_format == Some(lsp::InsertTextFormat::SNIPPET)
                    });
                    let detail = lsp_completion
                        .as_ref()
                        .and_then(|item| item.detail.clone());
                    let kind = lsp_completion
                        .as_ref()
                        .and_then(|item| item.kind)
                        .and_then(|kind| serde_json::to_value(kind).ok());
                    let documentation = lsp_completion
                        .as_ref()
                        .and_then(|item| item.documentation.as_ref())
                        .map(|documentation| match documentation {
                            lsp::Documentation::String(text) => text.clone(),
                            lsp::Documentation::MarkupContent(markup) => markup.value.clone(),
                        });
                    let sort_text = lsp_completion
                        .as_ref()
                        .and_then(|item| item.sort_text.clone());
                    json!({
                        "label": completion.label.text,
                        "insert_text": completion.new_text,
                        "filter_text": completion.source.filter_text(),
                        "start_offset": range.start,
                        "end_offset": range.end,
                        "is_snippet": is_snippet,
                        "detail": detail,
                        "kind": kind,
                        "documentation": documentation,
                        "sort_text": sort_text,
                    })
                })
                .collect::<Vec<_>>()
        });
        Ok(json!({ "completions": items }))
    }

    async fn definition(&self, buffer_id: u64, offset: usize, cx: &mut AsyncApp) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let links = self
            .project
            .update(cx, |project, cx| project.definitions(&buffer, offset, cx))
            .await?
            .unwrap_or_default();
        let locations = links
            .into_iter()
            .filter_map(|link| {
                link.target.buffer.read_with(cx, |target, cx| {
                    let path = target.file()?.as_local()?.abs_path(cx);
                    let snapshot = target.snapshot();
                    let range = link.target.range.start.to_offset(&snapshot)
                        ..link.target.range.end.to_offset(&snapshot);
                    let point = snapshot.offset_to_point(range.start);
                    Some(json!({
                        "buffer_id": u64::from(target.remote_id()),
                        "path": path.to_string_lossy(),
                        "start_offset": range.start,
                        "end_offset": range.end,
                        "line": point.row + 1,
                        "column": point.column + 1,
                    }))
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "locations": locations }))
    }

    /// Renames through the language server. Buffers the client has open come back as
    /// `changed_buffer_ids` for it to reload; files it doesn't have open would otherwise
    /// only change in this process's memory, so they are saved here.
    async fn rename(
        &self,
        buffer_id: u64,
        offset: usize,
        new_name: String,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let transaction = self
            .project
            .update(cx, |project, cx| {
                project.perform_rename(buffer, offset, new_name, None, cx)
            })
            .await?;
        let open_ids = self
            .buffers
            .lock()
            .map_err(|_| anyhow!("buffer table poisoned"))?
            .keys()
            .copied()
            .collect::<CollectionsHashSet<u64>>();
        let mut changed_buffer_ids = Vec::new();
        let mut saved_paths = Vec::new();
        for changed in transaction.0.into_keys() {
            let (id, path) = changed.read_with(cx, |buffer, cx| {
                (
                    u64::from(buffer.remote_id()),
                    buffer
                        .file()
                        .and_then(|file| file.as_local())
                        .map(|file| file.abs_path(cx).to_string_lossy().to_string()),
                )
            });
            if open_ids.contains(&id) {
                changed_buffer_ids.push(id);
            } else {
                self.project
                    .update(cx, |project, cx| project.save_buffer(changed, cx))
                    .await?;
                saved_paths.extend(path);
            }
        }
        Ok(json!({ "changed_buffer_ids": changed_buffer_ids, "saved_paths": saved_paths }))
    }

    async fn format(&self, buffer_id: u64, cx: &mut AsyncApp) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let mut buffers = CollectionsHashSet::default();
        buffers.insert(buffer.clone());
        self.project
            .update(cx, |project, cx| {
                project.format(buffers, LspFormatTarget::Buffers, true, FormatTrigger::Manual, cx)
            })
            .await?;
        let text = buffer.read_with(cx, |buffer, _| buffer.text());
        Ok(json!({ "text": text }))
    }

    async fn hover(&self, buffer_id: u64, offset: usize, cx: &mut AsyncApp) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let hovers = self
            .project
            .update(cx, |project, cx| project.hover(&buffer, offset, cx))
            .await
            .unwrap_or_default();
        let blocks = hovers
            .into_iter()
            .flat_map(|hover| hover.contents)
            .map(|block| block.text)
            .collect::<Vec<_>>();
        Ok(json!({ "contents": blocks }))
    }

    fn diagnostics(&self, buffer_id: u64, cx: &mut AsyncApp) -> Result<Value> {
        let buffer = self.buffer_for_id(buffer_id)?;
        let diagnostics = buffer.read_with(cx, |buffer, _| {
            let snapshot = buffer.snapshot();
            let len = snapshot.len();
            snapshot
                .diagnostics_in_range::<usize, usize>(0..len, false)
                .map(|entry| {
                    json!({
                        "start_offset": entry.range.start,
                        "end_offset": entry.range.end,
                        "severity": entry.diagnostic.severity,
                        "message": entry.diagnostic.message.as_str(),
                    })
                })
                .collect::<Vec<_>>()
        });
        Ok(json!({ "diagnostics": diagnostics }))
    }

    fn settings_schema(&self) -> Result<Value> {
        let language_names = self
            .languages
            .language_names()
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let lsp_adapter_names = self
            .languages
            .available_lsp_adapter_names()
            .into_iter()
            .map(|name| name.0.to_string())
            .collect::<Vec<_>>();
        let theme_names = self.theme_registry.list_names();
        let empty = CollectionsHashMap::default();
        let schema = SettingsStore::json_schema(&SettingsJsonSchemaParams {
            language_names: &language_names,
            font_names: &[],
            theme_names: &theme_names,
            icon_theme_names: &[],
            lsp_adapter_names: &lsp_adapter_names,
            action_names: &[],
            action_documentation: &empty,
            deprecations: &empty,
            deprecation_messages: &empty,
        });
        let defaults: Value = settings::parse_json_with_comments(&settings::default_settings())?;
        let mut themes = self
            .theme_registry
            .list()
            .into_iter()
            .map(|meta| {
                let appearance = match meta.appearance {
                    Appearance::Light => "light",
                    Appearance::Dark => "dark",
                };
                json!({ "name": meta.name.as_ref(), "appearance": appearance })
            })
            .collect::<Vec<_>>();
        themes.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(json!({ "schema": schema, "defaults": defaults, "themes": themes }))
    }

    fn set_user_settings(
        &self,
        content: String,
        appearance: Option<String>,
        cx: &mut AsyncApp,
    ) -> Result<Value> {
        let result = cx.update_global::<SettingsStore, _>(|store, cx| {
            store.set_user_settings(&content, cx)
        });
        let error = match result.parse_status {
            ParseStatus::Failed { error } => Some(error),
            ParseStatus::Success | ParseStatus::Unchanged => None,
        };
        let dark = appearance.as_deref() != Some("light");
        let theme = self.theme_for(&content, dark);
        *self.theme.lock().unwrap() = theme.clone();
        self.languages.set_theme(theme.clone());
        Ok(json!({ "theme": theme.name.as_ref(), "error": error }))
    }

    /// Zed's `theme` setting is a name, or `{mode, light, dark}` where `mode: "system"`
    /// follows the app's appearance. Unknown names fall back to One Light / One Dark.
    fn theme_for(&self, content: &str, dark: bool) -> Arc<Theme> {
        let value = settings::parse_json_with_comments::<Value>(content).ok();
        let name = match value.as_ref().and_then(|value| value.get("theme")) {
            Some(Value::String(name)) => Some(name.clone()),
            Some(Value::Object(theme)) => {
                let use_dark = match theme.get("mode").and_then(Value::as_str) {
                    Some("light") => false,
                    Some("dark") => true,
                    _ => dark,
                };
                theme
                    .get(if use_dark { "dark" } else { "light" })
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }
            _ => None,
        };
        let fallback = if dark { "One Dark" } else { "One Light" };
        name.and_then(|name| self.theme_registry.get(&name).ok())
            .or_else(|| self.theme_registry.get(fallback).ok())
            .unwrap_or_else(|| self.theme.lock().unwrap().clone())
    }

    fn worktree_for_id(&self, worktree_id: u64) -> Result<Entity<Worktree>> {
        self.worktrees
            .lock()
            .unwrap()
            .get(&worktree_id)
            .cloned()
            .context("no such worktree; open_folder must be called first")
    }

    fn buffer_for_id(&self, buffer_id: u64) -> Result<Entity<Buffer>> {
        self.buffers
            .lock()
            .unwrap()
            .get(&buffer_id)
            .map(|(buffer, _)| buffer.clone())
            .context("no such buffer; open_buffer must be called first")
    }
}

fn hsla_to_hex(color: gpui::Hsla) -> String {
    let rgba = color.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        (rgba.r * 255.0).round() as u8,
        (rgba.g * 255.0).round() as u8,
        (rgba.b * 255.0).round() as u8,
        (rgba.a * 255.0).round() as u8,
    )
}
