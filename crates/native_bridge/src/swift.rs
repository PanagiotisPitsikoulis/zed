//! Swift, which upstream Zed ships as an extension (`zed-extensions/swift`) rather than a
//! built-in language. This process doesn't run Zed's extension host, so the extension's
//! language files are compiled in here instead: its `config.toml` and queries (Apache-2.0,
//! licence beside them in `languages/swift/`), the `tree-sitter-swift` grammar it uses, and
//! its language server, `sourcekit-lsp`, which ships with Xcode and the Swift toolchain.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use futures::FutureExt as _;
use gpui::AsyncApp;
use language::{
    LanguageConfig, LanguageQueries, LanguageRegistry, LanguageServerName, LoadedLanguage,
    LspAdapter, LspAdapterDelegate, LspInstaller, QueryFile, QueryFileContents, Toolchain,
};
use lsp::LanguageServerBinary;

const QUERIES: &[(QueryFile, &str)] = &[
    (QueryFile::Highlights, include_str!("../languages/swift/highlights.scm")),
    (QueryFile::Brackets, include_str!("../languages/swift/brackets.scm")),
    (QueryFile::Indents, include_str!("../languages/swift/indents.scm")),
    (QueryFile::Injections, include_str!("../languages/swift/injections.scm")),
    (QueryFile::Outline, include_str!("../languages/swift/outline.scm")),
    (QueryFile::TextObjects, include_str!("../languages/swift/textobjects.scm")),
];

pub fn register(languages: &LanguageRegistry) {
    languages.register_native_grammars([("swift", tree_sitter_swift::LANGUAGE)]);
    let config = LanguageConfig::from_toml(include_str!("../languages/swift/config.toml"))
        .expect("the bundled Swift config.toml parses");
    languages.register_lsp_adapter(config.name.clone(), Arc::new(SourceKitLspAdapter));
    languages.register_language(
        config.name.clone(),
        config.grammar.clone(),
        config.matcher.clone(),
        config.hidden,
        None,
        Arc::new(move || {
            let config = config.clone();
            async move {
                Ok(LoadedLanguage {
                    config,
                    queries: LanguageQueries::from_files(QUERIES.iter().map(|(file, text)| {
                        QueryFileContents::new(*file, std::borrow::Cow::Borrowed(*text))
                    })),
                    context_provider: None,
                    toolchain_provider: None,
                    manifest_name: None,
                })
            }
            .boxed()
        }),
    );
}

/// `sourcekit-lsp` from the PATH (`/usr/bin/sourcekit-lsp` is Xcode's), never downloaded: it
/// has to match the toolchain that builds the project.
struct SourceKitLspAdapter;

impl SourceKitLspAdapter {
    const SERVER_NAME: LanguageServerName = LanguageServerName::new_static("sourcekit-lsp");

    async fn find(delegate: &dyn LspAdapterDelegate) -> Option<LanguageServerBinary> {
        let path = delegate.which(Self::SERVER_NAME.as_ref()).await?;
        Some(LanguageServerBinary {
            path,
            arguments: Vec::new(),
            env: None,
        })
    }
}

impl LspInstaller for SourceKitLspAdapter {
    type BinaryVersion = ();

    async fn check_if_user_installed(
        &self,
        delegate: &Arc<dyn LspAdapterDelegate>,
        _: Option<Toolchain>,
        _: &AsyncApp,
    ) -> Option<LanguageServerBinary> {
        Self::find(delegate.as_ref()).await
    }

    async fn fetch_latest_server_version(
        &self,
        _: &Arc<dyn LspAdapterDelegate>,
        _: bool,
        _: &mut AsyncApp,
    ) -> Result<()> {
        bail!("sourcekit-lsp comes with Xcode or the Swift toolchain; it isn't downloaded")
    }

    fn fetch_server_binary(
        &self,
        _: (),
        _: PathBuf,
        _: &Arc<dyn LspAdapterDelegate>,
    ) -> impl Send + Future<Output = Result<LanguageServerBinary>> + use<> {
        async { bail!("sourcekit-lsp comes with Xcode or the Swift toolchain; it isn't downloaded") }
    }

    async fn cached_server_binary(
        &self,
        _: PathBuf,
        delegate: &dyn LspAdapterDelegate,
    ) -> Option<LanguageServerBinary> {
        Self::find(delegate).await
    }
}

impl LspAdapter for SourceKitLspAdapter {
    fn name(&self) -> LanguageServerName {
        Self::SERVER_NAME
    }
}
