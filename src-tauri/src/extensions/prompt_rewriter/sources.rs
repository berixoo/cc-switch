//! Read-only, bounded source snapshots. No live prompt service or official writer is used.
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::path::Path;

use super::config::MAX_TEXT_BYTES;
use super::{Config, Route};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceState {
    Ready,
    Missing,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub path: Option<String>,
    pub content: Option<String>,
    pub state: SourceState,
    pub error: Option<String>,
    pub loaded_at: Option<u64>,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            path: None,
            content: None,
            state: SourceState::Missing,
            error: None,
            loaded_at: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sources {
    pub claude_desktop: Source,
    pub claude_code: Source,
}

impl Sources {
    pub(crate) fn get(&self, route: Route) -> &Source {
        match route {
            Route::ClaudeDesktop => &self.claude_desktop,
            Route::ClaudeCode => &self.claude_code,
        }
    }

    pub(crate) fn load(config: &Config) -> Self {
        Self {
            claude_desktop: load_route(config, Route::ClaudeDesktop),
            claude_code: load_route(config, Route::ClaudeCode),
        }
    }
}

/// Open without waiting for a Unix FIFO peer, then validate the opened descriptor rather than
/// preflight metadata (which could race with a file replacement). Reads remain bounded below.
pub(super) fn open_regular_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

pub(crate) fn read_source(path: &Path) -> Source {
    let mut source = Source {
        path: Some(path.to_string_lossy().into_owned()),
        loaded_at: Some(super::now_ms()),
        ..Source::default()
    };
    let result = (|| {
        let file = match open_regular_file(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot open source ({})", error.kind())),
        };
        let mut bytes = Vec::new();
        file.take((MAX_TEXT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read source ({})", error.kind()))?;
        if bytes.len() > MAX_TEXT_BYTES {
            return Err("source exceeds 256 KiB".into());
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "source is not UTF-8".into())
    })();
    match result {
        Ok(Some(content)) => {
            source.state = SourceState::Ready;
            source.content = Some(content);
        }
        Ok(None) => {}
        Err(error) => {
            source.state = SourceState::Error;
            source.error = Some(error);
        }
    }
    source
}

fn load_route(config: &Config, route: Route) -> Source {
    if let Some(path) = &config.routes.get(route).user_instructions.source_path {
        return read_source(Path::new(path));
    }
    // Unit tests never consult the developer's real HOME. Explicit test paths still exercise IO.
    #[cfg(test)]
    {
        let _ = route;
        return Source::default();
    }
    #[cfg(not(test))]
    let app = match route {
        Route::ClaudeDesktop | Route::ClaudeCode => crate::app_config::AppType::Claude,
    };
    #[cfg(not(test))]
    match crate::prompt_files::prompt_file_path(&app) {
        Ok(path) => read_source(&path),
        Err(_) => Source {
            state: SourceState::Error,
            error: Some("cannot resolve default instruction source path".into()),
            loaded_at: Some(super::now_ms()),
            ..Source::default()
        },
    }
}
