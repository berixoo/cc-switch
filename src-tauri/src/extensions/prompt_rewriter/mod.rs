//! Request-scoped instruction replacement. Only this extension's JSON is writable.
mod adapters;
pub mod commands;
mod config;
mod engine;
mod sources;

#[allow(unused_imports)]
pub use config::{
    Config, Route, RouteConfig, Routes, SystemConfig, UserInstructionsConfig, WireFormat,
};
pub use engine::{FileStatus, Preview};
pub use sources::{Source, SourceState, Sources};

use config::{Snapshot, MAX_CONFIG_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Manager;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ObservationState {
    #[default]
    NotCaptured,
    Waiting,
    Captured,
    Error,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub state: ObservationState,
    pub system: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub format: Option<WireFormat>,
    pub request_id: Option<String>,
    pub captured_at: Option<u64>,
    pub file_status: FileStatus,
    pub system_applied: bool,
    pub user_applied: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observations {
    pub claude_desktop: Observation,
    pub claude_code: Observation,
}

impl Observations {
    fn get_mut(&mut self, route: Route) -> &mut Observation {
        match route {
            Route::ClaudeDesktop => &mut self.claude_desktop,
            Route::ClaudeCode => &mut self.claude_code,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: bool,
    pub config: Config,
    pub path: String,
    pub error: Option<String>,
    pub sources: Sources,
    pub observations: Observations,
}

#[derive(Default)]
struct CaptureSlot {
    generation: u64,
    owner: Option<u64>,
}

struct RuntimeState {
    path: PathBuf,
    snapshot: Arc<Snapshot>,
    sources: Sources,
    observations: Observations,
    captures: [CaptureSlot; 2],
    epoch: u64,
    error: Option<String>,
}

impl RuntimeState {
    fn clear_observations(&mut self) {
        self.observations = Observations::default();
        self.captures = Default::default();
        self.epoch = self.epoch.wrapping_add(1);
    }
}

pub struct PromptRewriter {
    updates: Mutex<()>,
    state: Arc<RwLock<RuntimeState>>,
}

fn route_index(route: Route) -> usize {
    match route {
        Route::ClaudeDesktop => 0,
        Route::ClaudeCode => 1,
    }
}
fn config_path() -> PathBuf {
    crate::config::get_app_config_dir()
        .join("extensions")
        .join("prompt-rewriter.json")
}
fn default_snapshot() -> Arc<Snapshot> {
    Arc::new(Snapshot::compile(Config::default()).expect("valid default extension config"))
}

fn read_snapshot(path: &Path) -> Result<Arc<Snapshot>, String> {
    let file = match sources::open_regular_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(default_snapshot()),
        Err(error) => {
            return Err(format!(
                "prompt rewriter: cannot open config ({})",
                error.kind()
            ))
        }
    };
    let mut bytes = Vec::new();
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("prompt rewriter: cannot read config ({})", error.kind()))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("prompt rewriter: config exceeds 1 MiB".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "prompt rewriter: invalid config JSON at line {}, column {}",
            error.line(),
            error.column()
        )
    })?;
    // Detect the incompatible version before deserializing the removed fields. Never migrate/write.
    if value.get("version").and_then(Value::as_u64) == Some(1) {
        return Err("prompt rewriter: v1 is incompatible with instruction-file replacement; file preserved, explicitly save a v2 configuration".into());
    }
    let config = serde_json::from_value(value)
        .map_err(|_| "prompt rewriter: invalid v2 config schema (unknown fields are rejected)")?;
    Snapshot::compile(config).map(Arc::new)
}

fn ensure_available() -> Result<(), String> {
    if !cfg!(feature = "prompt-rewriter") {
        return Err(
            "prompt rewriter: unavailable in this build; rebuild with feature prompt-rewriter"
                .into(),
        );
    }
    Ok(())
}

impl PromptRewriter {
    pub fn load() -> Self {
        let path = config_path();
        if !cfg!(feature = "prompt-rewriter") {
            return Self::from_state(path, default_snapshot(), Sources::default(), None);
        }
        Self::load_at(path)
    }

    fn from_state(
        path: PathBuf,
        snapshot: Arc<Snapshot>,
        sources: Sources,
        error: Option<String>,
    ) -> Self {
        Self {
            updates: Mutex::new(()),
            state: Arc::new(RwLock::new(RuntimeState {
                path,
                snapshot,
                sources,
                error,
                observations: Observations::default(),
                captures: Default::default(),
                epoch: 0,
            })),
        }
    }

    fn load_at(path: PathBuf) -> Self {
        match read_snapshot(&path) {
            Ok(snapshot) => {
                let sources = Sources::load(&snapshot.config);
                Self::from_state(path, snapshot, sources, None)
            }
            Err(error) => {
                Self::from_state(path, default_snapshot(), Sources::default(), Some(error))
            }
        }
    }

    pub fn status(&self) -> Result<Status, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "prompt rewriter: state lock unavailable")?;
        Ok(Status {
            available: cfg!(feature = "prompt-rewriter"),
            config: state.snapshot.config.clone(),
            path: state.path.to_string_lossy().into_owned(),
            error: state.error.clone(),
            sources: state.sources.clone(),
            observations: state.observations.clone(),
        })
    }

    pub fn save(&self, config: Config) -> Result<Status, String> {
        ensure_available()?;
        self.save_with_path(config, config_path)
    }

    fn save_with_path(
        &self,
        config: Config,
        current_path: impl Fn() -> PathBuf,
    ) -> Result<Status, String> {
        let _update = self
            .updates
            .lock()
            .map_err(|_| "prompt rewriter: update lock unavailable")?;
        let path = current_path();
        let previous_path = self
            .state
            .read()
            .map_err(|_| "prompt rewriter: state lock unavailable")?
            .path
            .clone();
        if previous_path != path {
            self.invalidate_path(path)?;
            return Err("prompt rewriter: config directory changed; reload before saving".into());
        }
        let snapshot = Arc::new(Snapshot::compile(config)?);
        let sources = Sources::load(&snapshot.config);
        let destination = std::fs::canonicalize(&path).ok();
        for source in [&sources.claude_desktop, &sources.claude_code] {
            if source.path.as_deref().is_some_and(|source_path| {
                Path::new(source_path) == path.as_path()
                    || destination.as_ref().is_some_and(|destination| {
                        std::fs::canonicalize(source_path).ok().as_ref() == Some(destination)
                    })
            }) {
                return Err("prompt rewriter: source file aliases extension configuration; refusing to write a read-only source".into());
            }
        }
        let bytes = serde_json::to_vec_pretty(&snapshot.config)
            .map_err(|_| "prompt rewriter: config serialization failed")?;
        if current_path() != path {
            self.invalidate_path(current_path())?;
            return Err(
                "prompt rewriter: config directory changed during save; reload required".into(),
            );
        }
        crate::config::atomic_write_private(&path, &bytes).map_err(|_| {
            "prompt rewriter: atomic config write failed; previous snapshot retained"
        })?;
        if current_path() != path {
            self.invalidate_path(current_path())?;
            return Err(
                "prompt rewriter: config directory changed during save; reload required".into(),
            );
        }
        {
            let mut state = self
                .state
                .write()
                .map_err(|_| "prompt rewriter: state lock unavailable")?;
            state.snapshot = snapshot;
            state.sources = sources;
            state.error = None;
        }
        self.status()
    }

    pub fn reload(&self) -> Result<Status, String> {
        ensure_available()?;
        self.reload_with_path(config_path)
    }

    fn reload_with_path(&self, current_path: impl Fn() -> PathBuf) -> Result<Status, String> {
        let _update = self
            .updates
            .lock()
            .map_err(|_| "prompt rewriter: update lock unavailable")?;
        let path = current_path();
        let loaded = read_snapshot(&path).map(|snapshot| {
            let sources = Sources::load(&snapshot.config);
            (snapshot, sources)
        });
        if current_path() != path {
            self.invalidate_path(current_path())?;
            return Err(
                "prompt rewriter: config directory changed during reload; reload required".into(),
            );
        }
        {
            let mut state = self
                .state
                .write()
                .map_err(|_| "prompt rewriter: state lock unavailable")?;
            let switched = state.path != path;
            if switched {
                state.path = path;
                state.snapshot = default_snapshot();
                state.sources = Sources::default();
                state.clear_observations();
            }
            match loaded {
                Ok((snapshot, sources)) => {
                    state.snapshot = snapshot;
                    state.sources = sources;
                    state.error = None;
                }
                Err(error) => state.error = Some(error), // Same-path failures retain the whole valid revision.
            }
        }
        self.status()
    }

    fn invalidate_path(&self, path: PathBuf) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "prompt rewriter: state lock unavailable")?;
        state.path = path;
        state.snapshot = default_snapshot();
        state.sources = Sources::default();
        state.error = Some("prompt rewriter: config directory changed; reload required".into());
        state.clear_observations();
        Ok(())
    }

    pub fn refresh_sources(&self) -> Result<Status, String> {
        ensure_available()?;
        self.refresh_with_path(config_path)
    }

    fn refresh_with_path(&self, current_path: impl Fn() -> PathBuf) -> Result<Status, String> {
        let _update = self
            .updates
            .lock()
            .map_err(|_| "prompt rewriter: update lock unavailable")?;
        let path = current_path();
        let snapshot = {
            let state = self
                .state
                .read()
                .map_err(|_| "prompt rewriter: state lock unavailable")?;
            if state.path != path {
                drop(state);
                self.invalidate_path(path)?;
                return Err(
                    "prompt rewriter: config directory changed; reload before refreshing".into(),
                );
            }
            Arc::clone(&state.snapshot)
        };
        let sources = Sources::load(&snapshot.config);
        if current_path() != path {
            self.invalidate_path(current_path())?;
            return Err(
                "prompt rewriter: config directory changed during source refresh; reload required"
                    .into(),
            );
        }
        self.state
            .write()
            .map_err(|_| "prompt rewriter: state lock unavailable")?
            .sources = sources;
        self.status()
    }

    pub fn capture(&self, route: Route) -> Result<Status, String> {
        ensure_available()?;
        self.arm_with_current_path(route, config_path)
    }

    #[cfg(test)]
    fn arm_with_path(&self, route: Route, path: PathBuf) -> Result<Status, String> {
        self.arm_with_current_path(route, || path.clone())
    }

    fn arm_with_current_path(
        &self,
        route: Route,
        current_path: impl Fn() -> PathBuf,
    ) -> Result<Status, String> {
        let _update = self
            .updates
            .lock()
            .map_err(|_| "prompt rewriter: update lock unavailable")?;
        let mut state = self
            .state
            .write()
            .map_err(|_| "prompt rewriter: state lock unavailable")?;
        let path = current_path();
        if state.path != path {
            drop(state);
            self.invalidate_path(path)?;
            return Err("prompt rewriter: config directory changed; reload before capture".into());
        }
        let slot = &mut state.captures[route_index(route)];
        slot.generation = slot.generation.wrapping_add(1);
        slot.owner = None;
        *state.observations.get_mut(route) = Observation {
            state: ObservationState::Waiting,
            ..Observation::default()
        };
        drop(state);
        self.status()
    }

    #[cfg(test)]
    fn context_at(&self, route: Route, path: PathBuf) -> Result<Option<RewriteContext>, String> {
        self.context_with_path(route, || path.clone())
    }

    fn context_with_path(
        &self,
        route: Route,
        current_path: impl Fn() -> PathBuf,
    ) -> Result<Option<RewriteContext>, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "prompt rewriter: state lock unavailable")?;
        if state.path != current_path() {
            drop(state);
            let _update = self
                .updates
                .lock()
                .map_err(|_| "prompt rewriter: update lock unavailable")?;
            // A reload may have published another directory while this request waited.
            // Resolve again under the update lock; never invalidate it using a stale path.
            let path = current_path();
            if self
                .state
                .read()
                .map_err(|_| "prompt rewriter: state lock unavailable")?
                .path
                != path
            {
                self.invalidate_path(path)?;
            }
            return Ok(None);
        }
        let slot = &state.captures[route_index(route)];
        let capture = (slot.generation > 0 && slot.owner.is_none()).then_some(slot.generation);
        if !state.snapshot.active(route) && capture.is_none() {
            return Ok(None);
        }
        Ok(Some(RewriteContext {
            snapshot: Arc::clone(&state.snapshot),
            source: state.sources.get(route).clone(),
            route,
            runtime: Some(Arc::clone(&self.state)),
            epoch: state.epoch,
            capture,
            identity: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            client_model: None,
        }))
    }

    pub fn preview(
        &self,
        mut config: Config,
        route: Route,
        format: WireFormat,
        body: Value,
    ) -> Result<Preview, String> {
        ensure_available()?;
        if format != route.wire_format() {
            return Err(
                "prompt rewriter: preview format must match the route's ingress wire format".into(),
            );
        }
        config.enabled = true;
        match route {
            Route::ClaudeDesktop => config.routes.claude_desktop.enabled = true,
            Route::ClaudeCode => config.routes.claude_code.enabled = true,
        }
        let snapshot = Snapshot::compile(config)?;
        // Pure preview uses only the saved, cached source revision. A changed draft path cannot
        // silently trigger filesystem IO or claim the cached contents came from that other path.
        let source = {
            let state = self
                .state
                .read()
                .map_err(|_| "prompt rewriter: state lock unavailable")?;
            if snapshot
                .config
                .routes
                .get(route)
                .user_instructions
                .source_path
                == state
                    .snapshot
                    .config
                    .routes
                    .get(route)
                    .user_instructions
                    .source_path
            {
                state.sources.get(route).clone()
            } else {
                Source::default()
            }
        };
        engine::rewrite(&snapshot, route, format, &source, body)
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Clone)]
pub struct RewriteContext {
    snapshot: Arc<Snapshot>,
    source: Source,
    route: Route,
    runtime: Option<Arc<RwLock<RuntimeState>>>,
    epoch: u64,
    capture: Option<u64>,
    identity: u64,
    client_model: Option<Option<String>>,
}

impl RewriteContext {
    #[cfg(test)]
    pub fn for_test(config: Config, route: Route) -> Result<Self, String> {
        Self::for_test_with_source(config, route, Source::default())
    }

    #[cfg(test)]
    pub fn for_test_with_source(
        config: Config,
        route: Route,
        source: Source,
    ) -> Result<Self, String> {
        Ok(Self {
            snapshot: Arc::new(Snapshot::compile(config)?),
            source,
            route,
            runtime: None,
            epoch: 0,
            capture: None,
            identity: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            client_model: None,
        })
    }

    /// Preserve client metadata before Stack routing decodes the model identifier.
    pub fn with_client_model(mut self, body: &Value) -> Self {
        if self.capture.is_some() {
            // Keep enough to detect oversized metadata without retaining an unbounded string.
            self.client_model = Some(
                body.get("model")
                    .and_then(Value::as_str)
                    .map(|model| model.chars().take(1025).collect()),
            );
        }
        self
    }

    pub fn system_enabled(&self) -> bool {
        let target = &self.snapshot.config.routes.get(self.route).system;
        self.snapshot.active(self.route) && target.enabled && target.replacement.is_some()
    }

    pub fn user_instructions_enabled(&self) -> bool {
        let target = &self
            .snapshot
            .config
            .routes
            .get(self.route)
            .user_instructions;
        self.snapshot.active(self.route) && target.enabled && target.replacement.is_some()
    }

    /// Rewrite the client body once, before provider conversion, overrides, and retries.
    pub fn rewrite_ingress(&self, body: &mut Value) -> Result<(), String> {
        // Never retain a request body (chat history or credentials) in the request context.
        let result = engine::rewrite(
            &self.snapshot,
            self.route,
            self.route.wire_format(),
            &self.source,
            body.clone(),
        );
        self.observe(body, &result)?;
        // Capture-only or no-target contexts never reject or alter an otherwise normal request.
        if !self.snapshot.has_targets(self.route) {
            return Ok(());
        }
        let preview = result?;
        *body = preview.body;
        Ok(())
    }

    fn observe(&self, body: &Value, result: &Result<Preview, String>) -> Result<(), String> {
        let (Some(runtime), Some(generation)) = (&self.runtime, self.capture) else {
            return Ok(());
        };
        let mut state = runtime
            .write()
            .map_err(|_| "prompt rewriter: state lock unavailable")?;
        if state.epoch != self.epoch {
            return Ok(());
        }
        let slot = &mut state.captures[route_index(self.route)];
        if slot.generation != generation || slot.owner.is_some() {
            return Ok(());
        }
        slot.owner = Some(self.identity);
        let format = self.route.wire_format();
        let captured = engine::inspect_system(format, &self.source, body);
        // Capture only original client metadata. Provider selection and model mapping happen later.
        let model = self.client_model.as_ref().map_or_else(
            || body.get("model").and_then(Value::as_str),
            |model| model.as_deref(),
        );
        let metadata_too_large = model.is_some_and(|model| model.len() > 1024);
        let capture_error = if metadata_too_large {
            Some("prompt rewriter: observation metadata exceeds 1024 bytes".into())
        } else {
            captured
                .as_ref()
                .err()
                .cloned()
                .or_else(|| result.as_ref().err().cloned())
        };
        let error = capture_error.clone().or_else(|| {
            self.snapshot
                .active(self.route)
                .then(|| {
                    result
                        .as_ref()
                        .ok()
                        .and_then(|preview| preview.reason.clone())
                })
                .flatten()
        });
        *state.observations.get_mut(self.route) = Observation {
            state: if capture_error.is_some() {
                ObservationState::Error
            } else {
                ObservationState::Captured
            },
            system: captured.ok().flatten(),
            provider_id: None,
            model: (!metadata_too_large)
                .then(|| model.map(str::to_string))
                .flatten(),
            format: Some(format),
            request_id: Some(format!("pr-{}-{}", now_ms(), self.identity)),
            captured_at: Some(now_ms()),
            file_status: result
                .as_ref()
                .map_or(FileStatus::Unknown, |preview| preview.file_status),
            system_applied: result.as_ref().is_ok_and(|preview| preview.system_applied),
            user_applied: result.as_ref().is_ok_and(|preview| preview.user_applied),
            error,
        };
        Ok(())
    }
}

/// Freeze config/source snapshots for one ingress rewrite before provider-specific processing.
pub fn begin_request(
    app_handle: Option<&tauri::AppHandle>,
    route: Route,
    skip: bool,
) -> Result<Option<RewriteContext>, String> {
    if !cfg!(feature = "prompt-rewriter") || skip {
        return Ok(None);
    }
    let Some(state) = app_handle.and_then(|app| app.try_state::<PromptRewriter>()) else {
        return Ok(None);
    };
    state.context_with_path(route, config_path)
}

pub fn ensure_custom_update_allowed() -> Result<(), String> {
    if cfg!(feature = "prompt-rewriter") {
        return Err("This custom build includes prompt-rewriter. Rebuild from source to update; installing an official binary would remove the extension.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
