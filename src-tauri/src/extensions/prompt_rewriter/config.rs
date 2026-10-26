use serde::{Deserialize, Serialize};

pub(crate) const MAX_CONFIG_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_TEXT_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    ClaudeDesktop,
    ClaudeCode,
}

impl Route {
    pub fn wire_format(self) -> WireFormat {
        match self {
            Self::ClaudeDesktop | Self::ClaudeCode => WireFormat::Anthropic,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WireFormat {
    Anthropic,
    Responses,
    Chat,
    Gemini,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemConfig {
    pub enabled: bool,
    pub replacement: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserInstructionsConfig {
    pub enabled: bool,
    pub replacement: Option<String>,
    pub source_path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteConfig {
    pub enabled: bool,
    pub system: SystemConfig,
    pub user_instructions: UserInstructionsConfig,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Routes {
    pub claude_desktop: RouteConfig,
    pub claude_code: RouteConfig,
}

impl<'de> Deserialize<'de> for Routes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct CompatibleRoutes {
            claude_desktop: RouteConfig,
            #[serde(default)]
            claude_code: RouteConfig,
            // Accept only this removed v2 field; never retain or load its contents.
            #[serde(default, rename = "codex")]
            _removed_codex: serde::de::IgnoredAny,
        }

        let routes = CompatibleRoutes::deserialize(deserializer)?;
        Ok(Self {
            claude_desktop: routes.claude_desktop,
            claude_code: routes.claude_code,
        })
    }
}

impl Routes {
    pub(crate) fn get(&self, route: Route) -> &RouteConfig {
        match route {
            Route::ClaudeDesktop => &self.claude_desktop,
            Route::ClaudeCode => &self.claude_code,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub enabled: bool,
    pub routes: Routes,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 2,
            enabled: false,
            routes: Routes::default(),
        }
    }
}

/// Immutable configuration revision. Source snapshots are attached by the runtime, not read here.
pub(crate) struct Snapshot {
    pub config: Config,
}

impl Snapshot {
    pub(crate) fn compile(config: Config) -> Result<Self, String> {
        if config.version == 1 {
            return Err("prompt rewriter: v1 is incompatible with instruction-file replacement; file preserved, explicitly save a v2 configuration".into());
        }
        if config.version != 2 {
            return Err("prompt rewriter: unsupported config.version (expected 2)".into());
        }
        for route in [&config.routes.claude_desktop, &config.routes.claude_code] {
            for text in [
                &route.system.replacement,
                &route.user_instructions.replacement,
            ]
            .into_iter()
            .flatten()
            {
                if text.len() > MAX_TEXT_BYTES {
                    return Err("prompt rewriter: replacement exceeds 256 KiB".into());
                }
            }
            if let Some(path) = &route.user_instructions.source_path {
                if path.is_empty()
                    || path.len() > 4096
                    || path.chars().any(char::is_control)
                    || !std::path::Path::new(path).is_absolute()
                {
                    return Err("prompt rewriter: sourcePath must be an absolute path of at most 4096 bytes without control characters".into());
                }
            }
        }
        let bytes = serde_json::to_vec_pretty(&config)
            .map_err(|_| "prompt rewriter: config serialization failed")?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err("prompt rewriter: config exceeds 1 MiB".into());
        }
        Ok(Self { config })
    }

    pub(crate) fn active(&self, route: Route) -> bool {
        self.config.enabled && self.config.routes.get(route).enabled
    }

    pub(crate) fn has_targets(&self, route: Route) -> bool {
        let route_config = self.config.routes.get(route);
        self.active(route)
            && ((route_config.system.enabled && route_config.system.replacement.is_some())
                || (route_config.user_instructions.enabled
                    && route_config.user_instructions.replacement.is_some()))
    }
}
