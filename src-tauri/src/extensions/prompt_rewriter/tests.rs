use super::*;
use serde_json::json;
use std::sync::{Barrier, Mutex};

fn config(system: Option<&str>, user: Option<&str>) -> Config {
    let route = RouteConfig {
        enabled: true,
        system: SystemConfig {
            enabled: system.is_some(),
            replacement: system.map(str::to_string),
        },
        user_instructions: UserInstructionsConfig {
            enabled: user.is_some(),
            replacement: user.map(str::to_string),
            source_path: None,
        },
    };
    Config {
        enabled: true,
        routes: Routes {
            claude_desktop: route.clone(),
            claude_code: route,
        },
        ..Config::default()
    }
}

fn source(route: Route) -> Source {
    Source {
        path: Some(
            match route {
                Route::ClaudeDesktop | Route::ClaudeCode => "/home/me/.claude/CLAUDE.md",
            }
            .into(),
        ),
        content: Some("Original global instructions.\n第二行".into()),
        state: SourceState::Ready,
        error: None,
        loaded_at: Some(123),
    }
}

fn envelope(route: Route, source: &Source) -> String {
    let path = source.path.as_deref().unwrap();
    let text = source.content.as_deref().unwrap();
    match route {
        Route::ClaudeDesktop | Route::ClaudeCode => format!("<system-reminder>\nContents of {path} (user's private global instructions for all projects):\n\n{text}\n</system-reminder>"),
    }
}

fn run(config: Config, route: Route, format: WireFormat, source: &Source, body: Value) -> Preview {
    engine::rewrite(
        &Snapshot::compile(config).unwrap(),
        route,
        format,
        source,
        body,
    )
    .unwrap()
}

fn native_system(format: WireFormat, text: &str) -> (Value, &'static str) {
    match format {
        WireFormat::Anthropic => (
            json!({"system":text,"messages":[{"role":"user","content":"chat"}]}),
            "/system",
        ),
        WireFormat::Responses => (
            json!({"instructions":text,"input":[{"role":"user","content":"chat"}]}),
            "/instructions",
        ),
        WireFormat::Chat => (
            json!({"messages":[{"role":"developer","content":text},{"role":"user","content":"chat"}]}),
            "/messages/0/content",
        ),
        WireFormat::Gemini => (
            json!({"systemInstruction":{"parts":[{"text":text}]},"contents":[{"role":"user","parts":[{"text":"chat"}]}]}),
            "/systemInstruction/parts/0/text",
        ),
    }
}

const FORMATS: [WireFormat; 4] = [
    WireFormat::Anthropic,
    WireFormat::Responses,
    WireFormat::Chat,
    WireFormat::Gemini,
];

fn desktop_global_envelope(source: &Source) -> String {
    envelope(Route::ClaudeDesktop, source).replacen(
        "<system-reminder>\nContents of ",
        "<system-reminder>\nCodebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.\n\nContents of ",
        1,
    )
}

#[test]
fn desktop_global_first_block_can_share_a_message_with_context_and_chat() {
    let source = source(Route::ClaudeDesktop);
    let wrapper = desktop_global_envelope(&source);
    for format in FORMATS {
        let (mut body, system_path) = native_system(format, "Native system.");
        let (field, key, index, block_type) = match format {
            WireFormat::Anthropic => ("messages", "content", 0, "text"),
            WireFormat::Responses => ("input", "content", 0, "input_text"),
            WireFormat::Chat => ("messages", "content", 1, "text"),
            WireFormat::Gemini => ("contents", "parts", 0, "text"),
        };
        let blocks: Vec<_> = [
            wrapper.clone(),
            "<system-reminder>\ngit context\n</system-reminder>".into(),
            format!("ordinary chat quoting:\n{wrapper}"),
        ]
        .into_iter()
        .map(|text| {
            if format == WireFormat::Gemini {
                json!({"text":text})
            } else {
                json!({"type":block_type,"text":text})
            }
        })
        .collect();
        body[field][index][key] = json!(blocks);
        for replacement in ["New global.", ""] {
            let mut expected = body.clone();
            expected[field][index][key][0]["text"] =
                json!(wrapper.replace(source.content.as_deref().unwrap(), replacement,));
            *expected.pointer_mut(system_path).unwrap() = json!("New system.");
            let preview = run(
                config(Some("New system."), Some(replacement)),
                Route::ClaudeDesktop,
                format,
                &source,
                body.clone(),
            );
            assert_eq!(preview.body, expected);
            assert_eq!(preview.file_status, FileStatus::Identified);
            assert!(preview.system_applied && preview.user_applied);
            assert_eq!(preview.system_before.as_deref(), Some("Native system."));
        }
    }
}

#[test]
fn desktop_global_block_requires_exact_boundaries_source_and_leading_position() {
    let source = source(Route::ClaudeDesktop);
    let wrapper = desktop_global_envelope(&source);
    for body in [
        json!({"messages":[{"role":"user","content":[{"type":"text","text":format!("{wrapper}\nchat")},{"type":"text","text":"chat"}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"text","text":wrapper.replace("Codebase and user", "Changed preamble and user")},{"type":"text","text":"chat"}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"text","text":wrapper.replace("\n", "\r\n")},{"type":"text","text":"chat"}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"text","text":"chat"},{"type":"text","text":wrapper}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"text","text":wrapper},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}]}),
        json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":"result"},{"type":"text","text":wrapper}]}]}),
    ] {
        let preview = run(
            config(None, Some("wrong")),
            Route::ClaudeDesktop,
            WireFormat::Anthropic,
            &source,
            body.clone(),
        );
        assert_eq!(preview.body, body);
        assert!(!preview.user_applied);
    }
    // No usable carrier for this file: the replacement is injected instead of being dropped.
    for body in [
        json!({"messages":[{"role":"user","content":[{"type":"text","text":wrapper.replace("/home/me/", "/home/other/")},{"type":"text","text":"chat"}]}]}),
        json!({"messages":[{"role":"assistant","content":"history"},{"role":"user","content":[{"type":"text","text":wrapper},{"type":"text","text":"chat"}]}]}),
    ] {
        let preview = run(
            config(None, Some("replacement")),
            Route::ClaudeDesktop,
            WireFormat::Anthropic,
            &source,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Injected);
        assert!(preview.user_applied);
        assert!(preview
            .body
            .to_string()
            .contains("<cc-switch-user-instructions>"));
    }
    // A stale body at the same path, and repeated copies, are still this one file.
    for text in [
        wrapper.replace("Original global", "Stale global"),
        format!("{wrapper}\n{wrapper}"),
    ] {
        let body = json!({"messages":[{"role":"user","content":[
            {"type":"text","text":text},
            {"type":"text","text":"chat"}
        ]}]});
        let preview = run(
            config(None, Some("replacement")),
            Route::ClaudeDesktop,
            WireFormat::Anthropic,
            &source,
            body,
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.user_applied);
        assert!(!preview.body["messages"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Original global"));
    }
}

#[test]
fn mixed_leading_messages_and_tool_continuations_are_not_instruction_carriers() {
    let source = source(Route::ClaudeCode);
    let wrapper = envelope(Route::ClaudeCode, &source);
    let prose = format!("{wrapper}\nordinary chat");
    for (format, body) in [
        (
            WireFormat::Responses,
            json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AA=="},{"type":"input_text","text":wrapper}]}]}),
        ),
        (
            WireFormat::Responses,
            json!({"input":[{"role":"user","content":[{"type":"input_text","text":wrapper},{"type":"input_text","text":prose}]}]}),
        ),
        (
            WireFormat::Anthropic,
            json!({"messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":"result"},{"type":"text","text":wrapper}]}]}),
        ),
        (
            WireFormat::Gemini,
            json!({"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"image/png","data":"AA=="}},{"text":wrapper}]}]}),
        ),
    ] {
        let preview = run(
            config(None, Some("wrong")),
            Route::ClaudeCode,
            format,
            &source,
            body.clone(),
        );
        assert_eq!(preview.body, body);
        assert!(!preview.user_applied);
        assert_eq!(preview.file_status, FileStatus::Unidentified);
    }
}

#[test]
fn system_replacement_keeps_protected_envelopes_on_separate_lines() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let source = source(route);
        let global = envelope(route, &source);
        let mut project_source = source.clone();
        project_source.path = Some("/work/project/CLAUDE.md".into());
        project_source.content = Some("Project rules remain exactly.".into());
        let project = envelope(route, &project_source);
        for format in FORMATS {
            for replacement in ["New system.", ""] {
                let (body, path) = native_system(
                    format,
                    &format!("Old system.\n{global}\n\n{project}\nOld tail."),
                );
                let preview = run(
                    config(Some(replacement), Some("New file.")),
                    route,
                    format,
                    &source,
                    body,
                );
                let prefix = if replacement.is_empty() {
                    String::new()
                } else {
                    format!("{replacement}\n")
                };
                let expected = format!(
                    "{prefix}{}\n\n{project}",
                    global.replace(source.content.as_deref().unwrap(), "New file.")
                );
                assert_eq!(preview.body.pointer(path), Some(&json!(expected)));
                let inspected = run(
                    config(None, None),
                    route,
                    format,
                    &Source {
                        content: Some("New file.".into()),
                        ..source.clone()
                    },
                    preview.body,
                );
                assert_eq!(inspected.file_status, FileStatus::Identified);
            }
        }
    }
}

#[test]
fn posix_backslash_is_a_literal_path_character_not_a_directory_separator() {
    let mut source = source(Route::ClaudeDesktop);
    source.path = Some("/tmp/work/global/CLAUDE.md".into());
    let wrapper = envelope(Route::ClaudeDesktop, &source);
    let body = json!({"system":wrapper.replace("/work/global/", "/work\\global/"),"messages":[]});
    let preview = run(
        config(None, Some("wrong")),
        Route::ClaudeDesktop,
        WireFormat::Anthropic,
        &source,
        body.clone(),
    );
    // The literal backslash path is a different file, so its envelope keeps its bytes and the
    // replacement is injected as a request-level message instead.
    assert_eq!(preview.file_status, FileStatus::Injected);
    assert!(preview.user_applied);
    assert_eq!(preview.body["system"], body["system"]);
    assert!(preview
        .body
        .to_string()
        .contains("<cc-switch-user-instructions>"));
    source.path = Some("C:\\Users\\me\\.claude\\CLAUDE.md".into());
    let body = json!({"system":envelope(Route::ClaudeDesktop, &source).replace("C:\\Users\\me\\.claude\\CLAUDE.md", "c:/users/me/.claude/CLAUDE.md"),"messages":[]});
    assert!(
        run(
            config(None, Some("new")),
            Route::ClaudeDesktop,
            WireFormat::Anthropic,
            &source,
            body
        )
        .user_applied
    );
}

#[test]
fn schema_v2_is_exact_all_defaults_off_and_every_nested_object_rejects_unknown_fields() {
    let target = json!({"enabled":false,"replacement":null});
    let user = json!({"enabled":false,"replacement":null,"sourcePath":null});
    let route = json!({"enabled":false,"system":target,"userInstructions":user});
    let expected =
        json!({"version":2,"enabled":false,"routes":{"claudeDesktop":route,"claudeCode":route}});
    assert_eq!(serde_json::to_value(Config::default()).unwrap(), expected);
    for path in [
        "",
        "/routes",
        "/routes/claudeCode",
        "/routes/claudeCode/system",
        "/routes/claudeCode/userInstructions",
        "/routes/claudeDesktop",
        "/routes/claudeDesktop/system",
        "/routes/claudeDesktop/userInstructions",
    ] {
        let mut invalid = expected.clone();
        invalid
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(true));
        assert!(serde_json::from_value::<Config>(invalid).is_err(), "{path}");
    }
    assert_eq!(
        serde_json::to_value(Route::ClaudeDesktop).unwrap(),
        "claudeDesktop"
    );
    for (format, name) in FORMATS
        .into_iter()
        .zip(["anthropic", "responses", "chat", "gemini"])
    {
        assert_eq!(serde_json::to_value(format).unwrap(), name);
    }
    let mut invalid = Config::default();
    invalid.version = 1;
    assert!(Snapshot::compile(invalid).err().unwrap().contains("v1"));
    let mut invalid = config(Some(&"x".repeat(config::MAX_TEXT_BYTES + 1)), None);
    assert!(Snapshot::compile(invalid.clone()).is_err());
    invalid.routes.claude_desktop.system.replacement = None;
    invalid.routes.claude_code.system.replacement = None;
    invalid.routes.claude_code.user_instructions.source_path = Some("relative/CLAUDE.md".into());
    assert!(Snapshot::compile(invalid).is_err());
}

#[test]
fn older_v2_config_preserves_existing_routes_and_defaults_claude_code_off() {
    let original = config(Some("existing system"), Some("existing user"));
    let mut old = serde_json::to_value(&original).unwrap();
    old["routes"].as_object_mut().unwrap().remove("claudeCode");
    old["routes"]["codex"] = serde_json::to_value(&original.routes.claude_desktop).unwrap();
    let old_bytes = serde_json::to_vec_pretty(&old).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-rewriter.json");
    std::fs::write(&path, &old_bytes).unwrap();
    let runtime = PromptRewriter::load_at(path.clone());
    let loaded = runtime.status().unwrap();
    assert_eq!(loaded.error, None);
    assert_eq!(
        loaded.config.routes.claude_desktop,
        original.routes.claude_desktop
    );
    assert_eq!(loaded.config.routes.claude_code, RouteConfig::default());
    assert!(runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .is_none());
    assert_eq!(std::fs::read(path).unwrap(), old_bytes);
    assert_eq!(
        serde_json::to_value(Route::ClaudeCode).unwrap(),
        "claudeCode"
    );
    assert_eq!(Route::ClaudeDesktop.wire_format(), WireFormat::Anthropic);
    assert_eq!(Route::ClaudeCode.wire_format(), WireFormat::Anthropic);
}

#[test]
fn removed_codex_v2_field_is_ignored_without_loading_or_saving_its_source() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-rewriter.json");
    let claude_path = directory.path().join("CLAUDE.md");
    std::fs::write(&claude_path, "Claude source remains unchanged.").unwrap();
    let mut original = config(Some("Desktop system"), Some("Desktop file"));
    original.routes.claude_desktop.user_instructions.source_path =
        Some(claude_path.to_string_lossy().into_owned());
    original.routes.claude_code.system.replacement = Some("Code system".into());
    original.routes.claude_code.user_instructions.replacement = Some("Code file".into());
    original.routes.claude_code.user_instructions.source_path =
        Some(claude_path.to_string_lossy().into_owned());
    let mut old = serde_json::to_value(&original).unwrap();
    // This retired source aliases the config itself. Loading/validating it would break saving.
    old["routes"]["codex"] = json!({
        "enabled":true,
        "system":{"enabled":true,"replacement":"retired"},
        "userInstructions":{"enabled":true,"replacement":"retired", "sourcePath":path}
    });
    let bytes = serde_json::to_vec_pretty(&old).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let runtime = PromptRewriter::load_at(path.clone());
    let status = runtime.status().unwrap();
    assert_eq!(status.error, None);
    assert_eq!(status.config, original);
    assert_eq!(status.sources.claude_desktop.state, SourceState::Ready);
    assert_eq!(status.sources.claude_code.state, SourceState::Ready);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let serialized = serde_json::to_value(&status).unwrap();
    for pointer in ["/config/routes", "/sources", "/observations"] {
        assert!(serialized.pointer(pointer).unwrap().get("codex").is_none());
    }
    runtime
        .save_with_path(status.config, || path.clone())
        .unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved, serde_json::to_value(&original).unwrap());
    assert!(saved["routes"].get("codex").is_none());
    assert_eq!(
        std::fs::read_to_string(&claude_path).unwrap(),
        "Claude source remains unchanged."
    );
}

#[test]
fn only_the_removed_codex_field_is_ignored_and_codex_ipc_routes_are_rejected() {
    let original = config(Some("Claude system"), Some("Claude file"));
    for removed in [
        Value::Null,
        json!(true),
        json!({"unknown":"ignored", "sourcePath":"relative/retired.md"}),
    ] {
        let mut value = serde_json::to_value(&original).unwrap();
        value["routes"]["codex"] = removed;
        let loaded: Config = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(loaded, original);
        value["routes"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<Config>(value).is_err());
    }
    assert!(serde_json::from_value::<Route>(json!("codex")).is_err());
    for (name, route) in [
        ("claudeDesktop", Route::ClaudeDesktop),
        ("claudeCode", Route::ClaudeCode),
    ] {
        assert_eq!(serde_json::from_value::<Route>(json!(name)).unwrap(), route);
    }
}

#[test]
fn ingress_target_flags_require_active_enabled_and_non_null_replacements() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        for global_enabled in [false, true] {
            for route_enabled in [false, true] {
                for target_enabled in [false, true] {
                    for replacement in [None, Some(""), Some("replacement")] {
                        let mut configuration = config(replacement, replacement);
                        configuration.enabled = global_enabled;
                        let target = match route {
                            Route::ClaudeDesktop => &mut configuration.routes.claude_desktop,
                            Route::ClaudeCode => &mut configuration.routes.claude_code,
                        };
                        target.enabled = route_enabled;
                        target.system.enabled = target_enabled;
                        target.user_instructions.enabled = target_enabled;
                        let context = RewriteContext::for_test(configuration, route).unwrap();
                        let expected = global_enabled
                            && route_enabled
                            && target_enabled
                            && replacement.is_some();
                        assert_eq!(context.system_enabled(), expected);
                        assert_eq!(context.user_instructions_enabled(), expected);
                    }
                }
            }
        }
        for (system, user) in [(Some(""), None), (None, Some(""))] {
            let context = RewriteContext::for_test(config(system, user), route).unwrap();
            assert_eq!(context.system_enabled(), system.is_some());
            assert_eq!(context.user_instructions_enabled(), user.is_some());
        }
    }
}

#[test]
fn bare_user_text_equal_to_source_never_rewrites_in_any_protocol() {
    let source = source(Route::ClaudeCode);
    let original = source.content.clone().unwrap();
    for (format, body) in [
        (
            WireFormat::Anthropic,
            json!({"messages":[{"role":"user","content":original}]}),
        ),
        (
            WireFormat::Responses,
            json!({"input":[{"role":"user","content":[{"type":"input_text","text":original}]}]}),
        ),
        (WireFormat::Responses, json!({"input":original})),
        (
            WireFormat::Chat,
            json!({"messages":[{"role":"user","content":original}]}),
        ),
        (
            WireFormat::Gemini,
            json!({"contents":[{"role":"user","parts":[{"text":original}]}]}),
        ),
    ] {
        let preview = run(
            config(None, Some("new")),
            Route::ClaudeCode,
            format,
            &source,
            body.clone(),
        );
        // Bare text is never treated as the file, but a missing envelope now injects the
        // configured instructions as their own request-level message.
        assert_eq!(preview.file_status, FileStatus::Injected);
        assert!(preview.user_applied);
        assert!(preview
            .body
            .to_string()
            .contains("<cc-switch-user-instructions>"));
        assert!(preview
            .body
            .to_string()
            .contains("Original global instructions."));
    }
}

#[test]
fn only_complete_path_content_and_boundary_match_allows_file_replacement() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let source = source(route);
        let wrapper = envelope(route, &source);
        let body =
            json!({"input":[{"role":"user","content":[{"type":"input_text","text":wrapper}]}]});
        let preview = run(
            config(None, Some("replacement")),
            route,
            WireFormat::Responses,
            &source,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.user_applied);
        assert_eq!(
            preview.body["input"][0]["content"][0]["text"],
            wrapper.replace(source.content.as_deref().unwrap(), "replacement")
        );
        // Same absolute path and a complete envelope: an older snapshot is still this file.
        let stale = json!({"input":[{"role":"user","content":[{"type":"input_text","text":
            wrapper.replace(source.content.as_deref().unwrap(), "earlier snapshot")}]}]});
        let preview = run(
            config(None, Some("replacement")),
            route,
            WireFormat::Responses,
            &source,
            stale,
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.user_applied);
        assert!(!preview.body["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("earlier snapshot"));
        for malformed in [
            wrapper.replace("/home/me/", "relative/"),
            wrapper.replace("</system-reminder>", "</incomplete>"),
            format!("I quote this file:\n{wrapper}"),
            format!("{wrapper}\nthis is my chat, not a file carrier"),
            wrapper.replace('\n', "\r\n"),
        ] {
            let body = json!({"input":[{"role":"user","content":[{"type":"input_text","text":malformed}]}]});
            let preview = run(
                config(None, Some("wrong")),
                route,
                WireFormat::Responses,
                &source,
                body.clone(),
            );
            // A carrier for this file that cannot be separated blocks injection instead of
            // risking a second copy next to the unedited one.
            assert_eq!(preview.body, body);
            assert_eq!(preview.file_status, FileStatus::Unidentified);
            assert!(!preview.user_applied);
        }
        // A complete envelope for a different file is not this source: inject instead.
        let other = wrapper.replace("/home/me/", "/home/other/");
        let body =
            json!({"input":[{"role":"user","content":[{"type":"input_text","text":other}]}]});
        let preview = run(
            config(None, Some("replacement")),
            route,
            WireFormat::Responses,
            &source,
            body,
        );
        assert_eq!(preview.file_status, FileStatus::Injected);
        assert!(preview.user_applied);
        assert!(preview
            .body
            .to_string()
            .contains("<cc-switch-user-instructions>"));
    }
}

#[test]
fn combined_global_and_project_instruction_segments_preserve_project_bytes() {
    let source = source(Route::ClaudeCode);
    let global = envelope(Route::ClaudeCode, &source);
    let project = envelope(
        Route::ClaudeCode,
        &Source {
            path: Some("/work/project/CLAUDE.md".into()),
            content: Some("Project rules remain exactly.".into()),
            ..source.clone()
        },
    );
    let text = format!("{global}\n{project}");
    let body =
        json!({"input":[{"role":"user","content":text},{"role":"user","content":"ordinary chat"}]});
    let preview = run(
        config(None, Some("new global")),
        Route::ClaudeCode,
        WireFormat::Responses,
        &source,
        body,
    );
    assert!(preview.user_applied);
    assert_eq!(
        preview.body["input"][0]["content"],
        text.replace(source.content.as_deref().unwrap(), "new global")
    );
    assert_eq!(preview.body["input"][1]["content"], "ordinary chat");
}

#[test]
fn duplicate_file_segments_are_all_replaced_and_system_still_applies() {
    let source = source(Route::ClaudeCode);
    let wrapper = envelope(Route::ClaudeCode, &source);
    let body = json!({"instructions":format!("system\n{wrapper}\n{wrapper}"),"input":"chat"});
    let preview = run(
        config(Some("new system"), Some("new file")),
        Route::ClaudeCode,
        WireFormat::Responses,
        &source,
        body.clone(),
    );
    assert_eq!(preview.file_status, FileStatus::Identified);
    assert!(preview.system_applied && preview.user_applied);
    let rewritten = preview.body["instructions"].as_str().unwrap();
    assert!(rewritten.starts_with("new system"));
    assert_eq!(rewritten.matches("new file").count(), 2);
    assert!(!rewritten.contains("Original global instructions."));
    assert_eq!(preview.body["input"], body["input"]);
}

#[test]
fn resumed_conversation_envelope_is_found_after_leading_chat() {
    let source = source(Route::ClaudeCode);
    let wrapper = envelope(Route::ClaudeCode, &source);
    let body = json!({"system":"client system","messages":[
        {"role":"user","content":"first chat of the resumed session"},
        {"role":"assistant","content":"answer"},
        {"role":"user","content":"continue"},
        {"role":"assistant","content":"more"},
        {"role":"user","content":wrapper},
        {"role":"assistant","content":"acknowledged"}
    ]});
    let preview = run(
        config(Some("new system"), Some("new file")),
        Route::ClaudeCode,
        WireFormat::Anthropic,
        &source,
        body.clone(),
    );
    assert_eq!(preview.file_status, FileStatus::Identified);
    assert!(preview.user_applied && preview.system_applied);
    let rewritten = preview.body["messages"][4]["content"].as_str().unwrap();
    assert!(rewritten.contains("new file"));
    assert!(!rewritten.contains("Original global instructions."));
    for index in [0, 1, 2, 3, 5] {
        assert_eq!(preview.body["messages"][index], body["messages"][index]);
    }
}

#[test]
fn system_carried_envelope_is_replaced_from_a_stale_snapshot() {
    let current = source(Route::ClaudeCode);
    let stale = Source {
        content: Some("Instructions captured when the session started.".into()),
        ..current.clone()
    };
    let wrapper = desktop_global_envelope(&stale);
    // Claude Code embeds the global file in the system prompt, together with the rest of it.
    let body = json!({
        "system": format!("{wrapper}\n\nYou are Claude Code."),
        "messages":[{"role":"user","content":"continue the old task"}]
    });
    let preview = run(
        config(Some("new system"), Some("new file")),
        Route::ClaudeCode,
        WireFormat::Anthropic,
        &current,
        body.clone(),
    );
    assert_eq!(preview.file_status, FileStatus::Identified);
    assert!(preview.user_applied, "{:?}", preview.reason);
    assert_eq!(preview.body["messages"], body["messages"]);
    let system = preview.body["system"].as_str().unwrap();
    assert!(system.contains("new file"));
    assert!(!system.contains("captured when the session started"));
}

#[test]
fn resumed_conversation_without_any_envelope_injects_request_level_instructions() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let source = source(route);
        let body = json!({
            "system": "client system",
            "messages": [
                {"role":"user","content":"This session is being continued from a previous one."},
                {"role":"assistant","content":"understood"},
                {"role":"user","content":"继续"}
            ]
        });
        let preview = run(
            config(Some("new system"), Some("new file")),
            route,
            WireFormat::Anthropic,
            &source,
            body.clone(),
        );
        assert!(preview.system_applied && preview.user_applied);
        assert_eq!(preview.file_status, FileStatus::Injected);
        assert_eq!(preview.body["system"], json!("new system"));
        let messages = preview.body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        assert!(messages[0]
            .to_string()
            .contains("<cc-switch-user-instructions>"));
        // The client's own history keeps its bytes and order.
        for index in 0..3 {
            assert_eq!(messages[index + 1], body["messages"][index]);
        }
    }
}

#[test]
fn injected_request_instructions_are_idempotent() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let source = source(route);
        let (body, _) = native_system(route.wire_format(), "client system");
        let once = run(
            config(None, Some("new file")),
            route,
            route.wire_format(),
            &source,
            body,
        );
        assert_eq!(once.file_status, FileStatus::Injected);
        // Feeding the rewritten body back must not stack a second copy.
        let twice = run(
            config(None, Some("new file")),
            route,
            route.wire_format(),
            &source,
            once.body.clone(),
        );
        assert_eq!(twice.body, once.body);
        assert_eq!(
            twice.body["messages"]
                .to_string()
                .matches("<cc-switch-user-instructions>")
                .count(),
            1
        );
    }
}

#[test]
fn injected_instructions_follow_a_leading_system_turn() {
    let source = source(Route::ClaudeCode);
    let body = json!({"messages":[
        {"role":"system","content":"provider rules"},
        {"role":"user","content":"chat"}
    ]});
    let preview = run(
        config(None, Some("new file")),
        Route::ClaudeCode,
        WireFormat::Chat,
        &source,
        body.clone(),
    );
    assert_eq!(preview.file_status, FileStatus::Injected);
    let messages = preview.body["messages"].as_array().unwrap();
    // Chat payloads must still start with the system turn.
    assert_eq!(messages[0]["role"], "system");
    assert!(messages[1]
        .to_string()
        .contains("<cc-switch-user-instructions>"));
    assert_eq!(messages[2], body["messages"][1]);
}

#[test]
fn resumed_conversation_stale_snapshot_is_replaced_without_touching_chat() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let current = source(route);
        let stale = Source {
            content: Some("Original global instructions from the session start.".into()),
            ..current.clone()
        };
        let wrapper = envelope(route, &stale);
        let field = "messages";
        let (mut body, _) = native_system(route.wire_format(), "client system");
        body[field]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"role":"user","content":wrapper}));
        let preview = run(
            config(Some("new system"), Some("new file")),
            route,
            route.wire_format(),
            &current,
            body,
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.system_applied && preview.user_applied);
        assert_eq!(preview.body["system"], json!("new system"));
        let rewritten = preview.body[field][0]["content"].as_str().unwrap();
        assert!(rewritten.contains("new file"));
        assert!(!rewritten.contains("from the session start"));
        assert_eq!(preview.body[field][1]["content"], "chat");
    }
}

#[test]
fn chat_history_tools_media_assistant_and_quoted_wrappers_are_opaque() {
    let source = source(Route::ClaudeCode);
    let wrapper = envelope(Route::ClaudeCode, &source);
    let body = json!({"input":[
        {"role":"user","content":"actual ordinary first chat"},
        {"role":"assistant","content":wrapper},
        {"role":"user","content":wrapper},
        {"type":"function_call_output","output":wrapper},
        {"type":"compaction","role":"user","content":wrapper,"encrypted_content":"opaque"},
        {"role":"user","content":[{"type":"input_image","image_url":wrapper},{"type":"tool_result","content":[{"type":"input_text","text":wrapper}]}]}
    ]});
    let preview = run(
        config(None, Some("new file")),
        Route::ClaudeCode,
        WireFormat::Responses,
        &source,
        body.clone(),
    );
    // A user message that is nothing but file envelopes is the file's instructions wherever it
    // sits, which is how resumed sessions replay the global file.
    let rewritten = preview.body["input"][2]["content"].as_str().unwrap();
    assert!(rewritten.contains("new file"));
    assert!(!rewritten.contains("Original global instructions."));
    // Assistant turns, tool output, compaction state and media keep their bytes.
    for index in [0, 1, 3, 4, 5] {
        assert_eq!(preview.body["input"][index], body["input"][index]);
    }
    assert!(preview.user_applied);
    // A wrapper quoted inside chat prose is not a carrier.
    let quoted =
        json!({"messages":[{"role":"user","content":format!("I pasted it here:\n{wrapper}")}]});
    let preview = run(
        config(None, Some("wrong")),
        Route::ClaudeCode,
        WireFormat::Chat,
        &source,
        quoted.clone(),
    );
    assert_eq!(preview.body, quoted);
    assert!(!preview.user_applied);
}

#[test]
fn system_and_file_full_replacements_are_disjoint_in_all_four_wire_formats() {
    for format in FORMATS {
        let source = source(Route::ClaudeCode);
        let wrapper = envelope(Route::ClaudeCode, &source);
        let (body, path) = native_system(
            format,
            &format!("Original system.\n{wrapper}\nSystem tail."),
        );
        let preview = run(
            config(Some("New system."), Some("New file.")),
            Route::ClaudeCode,
            format,
            &source,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.system_applied && preview.user_applied);
        assert_eq!(
            preview.system_before,
            Some("Original system.\n\nSystem tail.".into())
        );
        assert_eq!(preview.system_after, Some("New system.".into()));
        assert_eq!(
            preview.body.pointer(path).unwrap(),
            &json!(format!(
                "New system.\n{}",
                wrapper.replace(source.content.as_deref().unwrap(), "New file.")
            ))
        );
        let keep_file = run(
            config(Some("System only."), None),
            Route::ClaudeCode,
            format,
            &source,
            body,
        );
        assert_eq!(
            keep_file.body.pointer(path).unwrap(),
            &json!(format!("System only.\n{wrapper}"))
        );
        assert!(!keep_file.user_applied);
    }
}

#[test]
fn a_file_only_system_carrier_is_preserved_when_system_body_is_created() {
    for format in FORMATS {
        let source = source(Route::ClaudeCode);
        let wrapper = envelope(Route::ClaudeCode, &source);
        let (body, path) = native_system(format, &wrapper);
        let preview = run(
            config(Some("New system"), None),
            Route::ClaudeCode,
            format,
            &source,
            body,
        );
        assert_eq!(
            preview.body.pointer(path).unwrap(),
            &json!(format!("New system\n{wrapper}"))
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
    }
}

#[test]
fn claude_file_wrapper_may_omit_exactly_one_terminal_lf() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let mut source = source(route);
        source.content.as_mut().unwrap().push('\n');
        let sent = Source {
            content: Some(
                source
                    .content
                    .as_deref()
                    .unwrap()
                    .strip_suffix('\n')
                    .unwrap()
                    .into(),
            ),
            ..source.clone()
        };
        let wrapper = desktop_global_envelope(&sent);
        let body = json!({
            "system":"Recalled memories inside `<system-reminder>` blocks are background context.",
            "messages":[{"role":"user","content":[
                {"type":"text","text":wrapper},
                {"type":"text","text":"context and chat remain unchanged"}
            ]}]
        });
        let preview = run(
            config(Some("New system."), Some("New file.")),
            route,
            WireFormat::Anthropic,
            &source,
            body.clone(),
        );
        assert!(
            preview.system_applied && preview.user_applied,
            "{:?}",
            preview.reason
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert_eq!(preview.body["system"], "New system.");
        assert_eq!(
            preview.body["messages"][0]["content"][1],
            body["messages"][0]["content"][1]
        );
        assert_eq!(
            preview.body["messages"][0]["content"][0]["text"],
            wrapper.replace(sent.content.as_deref().unwrap(), "New file.")
        );

        for content in [
            format!("{} \n", sent.content.as_deref().unwrap()),
            format!("{}\n\n", sent.content.as_deref().unwrap()),
            "Different source.\n".into(),
        ] {
            // The path and the complete envelope are unchanged, so this is the same file
            // with an older body: replace it instead of treating the session as unidentified.
            let stale = Source {
                content: Some(content),
                ..source.clone()
            };
            let replaced = run(
                config(None, Some("New file.")),
                route,
                WireFormat::Anthropic,
                &stale,
                body.clone(),
            );
            assert!(replaced.user_applied);
            assert!(!replaced.body["messages"][0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(sent.content.as_deref().unwrap()));
        }
        // A different path is a different file: its envelope is never rewritten, but the
        // configured replacement still applies as an injected request-level message.
        let other = Source {
            path: Some("/home/me/.claude/OTHER.md".into()),
            ..source.clone()
        };
        let injected = run(
            config(None, Some("wrong")),
            route,
            WireFormat::Anthropic,
            &other,
            body.clone(),
        );
        assert_eq!(injected.file_status, FileStatus::Injected);
        assert!(injected.user_applied);
        assert_eq!(
            injected.body["messages"][1], body["messages"][0],
            "the other file's envelope must keep its bytes"
        );
        assert!(injected
            .body
            .to_string()
            .contains("<cc-switch-user-instructions>"));
    }
}

#[test]
fn actual_system_file_boundaries_still_fail_closed() {
    let source = source(Route::ClaudeDesktop);
    for text in [
        "System.\n<system-reminder>\nUnknown file without a complete boundary",
        "System.\n  <system-reminder>incomplete",
        "System.\n<system-reminder>\r\nUnknown file",
        "System.\n</system-reminder>",
        source.content.as_deref().unwrap(),
    ] {
        let (body, _) = native_system(WireFormat::Anthropic, text);
        let preview = run(
            config(Some("wrong"), Some("wrong")),
            Route::ClaudeDesktop,
            WireFormat::Anthropic,
            &source,
            body.clone(),
        );
        assert!(!preview.system_applied && !preview.user_applied);
        assert_eq!(preview.body, body);
    }
}

#[test]
fn anthropic_ambiguous_shared_sources_remain_unchanged() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        for source_text in [
            "Private global instructions.\n",
            "First rule.\n</system-reminder>\nSecond rule.",
        ] {
            let selected = Source {
                content: Some(source_text.into()),
                ..source(route)
            };
            let text = if let Some(content) = source_text.strip_suffix('\n') {
                format!("Native system.\n{content}")
            } else {
                format!("Native system.\n{}", desktop_global_envelope(&selected))
            };
            let (body, _) = native_system(WireFormat::Anthropic, &text);
            for user in [None, Some("New file.")] {
                let preview = run(
                    config(Some("New system."), user),
                    route,
                    WireFormat::Anthropic,
                    &selected,
                    body.clone(),
                );
                assert!(!preview.system_applied && !preview.user_applied);
                assert_eq!(preview.body, body);
            }
        }
    }
}

#[test]
fn anthropic_system_replacement_does_not_depend_on_a_separate_file_match() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let original = source(route);
        let wrapper = desktop_global_envelope(&original);
        for selected_source in [
            original.clone(),
            Source {
                content: Some("Updated file not yet loaded by the client.".into()),
                ..original.clone()
            },
            Source::default(),
        ] {
            for file_present in [false, true] {
                let content = if file_present {
                    json!([
                        {"type":"text","text":wrapper},
                        {"type":"text","text":"context and ordinary chat"}
                    ])
                } else {
                    json!([{"type":"text","text":"ordinary chat"}])
                };
                let body = json!({
                    "system":[{"type":"text","text":"Original system."}],
                    "messages":[{"role":"user","content":content}],
                    "tools":[{"name":"unchanged","input_schema":{"type":"object"}}]
                });
                for replacement in ["New system.", ""] {
                    let preview = run(
                        config(Some(replacement), Some("New file.")),
                        route,
                        WireFormat::Anthropic,
                        &selected_source,
                        body.clone(),
                    );
                    assert!(preview.system_applied, "{:?}", preview.reason);
                    assert_eq!(preview.body["tools"], body["tools"]);
                    let loaded = selected_source.content.is_some();
                    assert_eq!(preview.user_applied, loaded);
                    if loaded {
                        assert_eq!(
                            preview.file_status,
                            if file_present {
                                FileStatus::Identified
                            } else {
                                FileStatus::Injected
                            }
                        );
                    } else {
                        assert_eq!(preview.file_status, FileStatus::Unidentified);
                        assert_eq!(preview.body["messages"], body["messages"]);
                        assert!(preview.reason.is_some());
                    }
                    if replacement.is_empty() {
                        assert!(preview.body.get("system").is_none());
                    } else {
                        assert_eq!(preview.body["system"][0]["text"], replacement);
                    }
                }
            }
        }
    }
}

#[test]
fn unidentified_shared_carriers_fail_closed_without_partial_system_replacement_or_append() {
    let source = source(Route::ClaudeCode);
    for text in [
        source.content.clone().unwrap(),
        "system\n<system-reminder>unknown origin</system-reminder>".into(),
    ] {
        let (body, _) = native_system(WireFormat::Responses, &text);
        let preview = run(
            config(Some("new system"), Some("new file")),
            Route::ClaudeCode,
            WireFormat::Responses,
            &source,
            body.clone(),
        );
        assert_eq!(preview.body, body);
        assert!(!preview.system_applied && !preview.user_applied);
        assert_eq!(preview.file_status, FileStatus::Unidentified);
        assert!(preview.reason.is_some());
    }
    // A carrier with no provenance is not a file match: the system target still applies and the
    // file replacement is added as a request-level instruction message.
    let (body, _) = native_system(
        WireFormat::Responses,
        "plain combined carrier without provenance",
    );
    let preview = run(
        config(Some("new system"), Some("new file")),
        Route::ClaudeCode,
        WireFormat::Responses,
        &source,
        body.clone(),
    );
    assert!(preview.system_applied && preview.user_applied);
    assert_eq!(preview.file_status, FileStatus::Injected);
    assert_eq!(preview.body["instructions"], json!("new system"));
    assert!(preview
        .body
        .to_string()
        .contains("<cc-switch-user-instructions>"));
    let (body, _) = native_system(WireFormat::Responses, &source.content.clone().unwrap());
    assert_eq!(
        run(
            config(Some("wrong"), None),
            Route::ClaudeCode,
            WireFormat::Responses,
            &source,
            body.clone()
        )
        .body,
        body
    );
}

#[test]
fn missing_empty_or_unreadable_source_is_not_a_file_match() {
    let original = source(Route::ClaudeCode);
    let body = json!({"input":[{"role":"user","content":envelope(Route::ClaudeCode,&original)}]});
    for source in [
        Source::default(),
        Source {
            state: SourceState::Error,
            error: Some("read failed".into()),
            ..original.clone()
        },
    ] {
        let preview = run(
            config(None, Some("wrong")),
            Route::ClaudeCode,
            WireFormat::Responses,
            &source,
            body.clone(),
        );
        assert_eq!(preview.body, body);
        assert_eq!(preview.file_status, FileStatus::Unidentified);
    }
}

#[test]
fn system_aggregate_replacement_preserves_nontext_and_cleans_deleted_cache_nodes() {
    let body = json!({"instructions":"first","input":[
        {"role":"system","content":"second"},
        {"role":"developer","content":[{"type":"input_text","text":"third","cache_control":{"type":"ephemeral"}},{"type":"input_image","image_url":"keep"}]},
        {"type":"compaction","role":"developer","content":"opaque","encrypted_content":"keep"},
        {"role":"user","content":"chat"}
    ]});
    let preview = run(
        config(Some("new"), None),
        Route::ClaudeCode,
        WireFormat::Responses,
        &Source::default(),
        body,
    );
    assert_eq!(
        preview.body,
        json!({"instructions":"new","input":[
            {"role":"developer","content":[{"type":"input_image","image_url":"keep"}]},
            {"type":"compaction","role":"developer","content":"opaque","encrypted_content":"keep"},
            {"role":"user","content":"chat"}
        ]})
    );
    let body = json!({"system":[{"type":"text","text":"old","cache_control":{"type":"ephemeral"},"custom":"keep"},{"type":"text","text":"tail","cache_control":{"type":"ephemeral"}},{"type":"unknown","data":"keep"}]});
    let preview = run(
        config(Some("new"), None),
        Route::ClaudeDesktop,
        WireFormat::Anthropic,
        &Source::default(),
        body,
    );
    assert_eq!(
        preview.body,
        json!({"system":[{"type":"text","text":"new","custom":"keep"},{"type":"unknown","data":"keep"}]})
    );
}

#[test]
fn null_disabled_and_empty_replacements_are_different() {
    for format in FORMATS {
        let (body, _) = native_system(format, "old");
        let mut null = config(None, None);
        null.routes.claude_code.system.enabled = true;
        let preview = run(
            null,
            Route::ClaudeCode,
            format,
            &Source::default(),
            body.clone(),
        );
        assert_eq!(preview.body, body);
        assert!(!preview.system_applied);
        let mut disabled = config(Some("new"), None);
        disabled.routes.claude_code.system.enabled = false;
        assert_eq!(
            run(
                disabled,
                Route::ClaudeCode,
                format,
                &Source::default(),
                body.clone()
            )
            .body,
            body
        );
        let preview = run(
            config(Some(""), None),
            Route::ClaudeCode,
            format,
            &Source::default(),
            body.clone(),
        );
        assert!(preview.system_applied);
        assert_eq!(preview.system_after, None);
        assert!(!preview.body.to_string().contains("old"));
        assert!(preview.body.to_string().contains("chat"));
        let mut global_off = config(Some("new"), None);
        global_off.enabled = false;
        let mut route_off = config(Some("new"), None);
        route_off.routes.claude_code.enabled = false;
        for disabled in [global_off, route_off] {
            assert_eq!(
                run(
                    disabled,
                    Route::ClaudeCode,
                    format,
                    &Source::default(),
                    body.clone()
                )
                .body,
                body
            );
        }
    }
    let source = source(Route::ClaudeCode);
    let wrapper = envelope(Route::ClaudeCode, &source);
    let body = json!({"instructions":"","input":[{"role":"user","content":wrapper}]});
    let preview = run(
        config(None, Some("")),
        Route::ClaudeCode,
        WireFormat::Responses,
        &source,
        body,
    );
    assert!(preview.user_applied);
    assert_eq!(preview.body["instructions"], ""); // System-disabled file edits do not clean unrelated carriers.
    assert_eq!(
        preview.body["input"][0]["content"],
        wrapper.replace(source.content.as_deref().unwrap(), "")
    );
}

#[test]
fn absent_native_system_can_be_created_without_touching_chat() {
    for (format, expected) in [
        (WireFormat::Anthropic, json!({"system":"new"})),
        (WireFormat::Responses, json!({"instructions":"new"})),
        (
            WireFormat::Chat,
            json!({"messages":[{"role":"system","content":"new"}]}),
        ),
        (
            WireFormat::Gemini,
            json!({"systemInstruction":{"parts":[{"text":"new"}]}}),
        ),
    ] {
        let preview = run(
            config(Some("new"), None),
            Route::ClaudeCode,
            format,
            &Source::default(),
            json!({}),
        );
        assert_eq!(preview.body, expected);
        assert_eq!(preview.system_before, None);
    }
}

#[test]
fn empty_sources_inject_separate_instructions_without_changing_chat() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let source = Source {
            content: Some(String::new()),
            ..source(route)
        };
        for system in [None, Some("new system"), Some("")] {
            let (mut body, _) = native_system(route.wire_format(), "old system");
            let field = "messages";
            body[field]
                .as_array_mut()
                .unwrap()
                .push(json!({"role":"assistant","content":"unchanged history"}));
            body[field].as_array_mut().unwrap().push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"opaque"},{"type":"image","source":{"type":"base64","data":"opaque"}}]}));
            body["tools"] = json!([{"name":"keep"}]);
            let preview = run(
                config(system, Some("new file")),
                route,
                route.wire_format(),
                &source,
                body.clone(),
            );
            assert!(preview.user_applied);
            assert_eq!(preview.system_applied, system.is_some());
            assert_eq!(preview.file_status, FileStatus::Injected);
            assert_eq!(preview.reason, None);
            let mut remaining = preview.body.clone();
            let injected = remaining[field].as_array_mut().unwrap().remove(0);
            assert_eq!(injected["role"], "user");
            assert_eq!(
                injected["content"][0]["text"],
                "<cc-switch-user-instructions>\nnew file\n</cc-switch-user-instructions>"
            );
            let expected = run(
                config(system, None),
                route,
                route.wire_format(),
                &source,
                body.clone(),
            );
            assert_eq!(remaining, expected.body);
            let again = run(
                config(system, Some("new file")),
                route,
                route.wire_format(),
                &source,
                preview.body.clone(),
            );
            assert_eq!(
                again.body, preview.body,
                "same explicit addition must not accumulate"
            );
        }
    }
}

#[test]
fn claude_empty_source_injection_supports_all_upstream_carriers() {
    let route = Route::ClaudeCode;
    let empty = Source {
        content: Some(String::new()),
        ..source(route)
    };
    for format in FORMATS {
        let (body, _) = native_system(format, "original system");
        let preview = run(
            config(Some("new system"), Some("new file")),
            route,
            format,
            &empty,
            body.clone(),
        );
        assert!(preview.system_applied && preview.user_applied);
        assert_eq!(preview.file_status, FileStatus::Injected);
        let field = match format {
            WireFormat::Anthropic | WireFormat::Chat => "messages",
            WireFormat::Responses => "input",
            WireFormat::Gemini => "contents",
        };
        let mut remaining = preview.body.clone();
        let entries = remaining[field].as_array_mut().unwrap();
        let position = entries
            .iter()
            .position(|entry| entry.to_string().contains("<cc-switch-user-instructions>"))
            .expect("injected instruction message");
        // Chat keeps its leading developer turn first; other carriers inject at the front.
        assert_eq!(position, usize::from(matches!(format, WireFormat::Chat)));
        let injected = entries.remove(position);
        assert_eq!(injected["role"], "user");
        assert_eq!(
            remaining,
            run(
                config(Some("new system"), None),
                route,
                format,
                &empty,
                body
            )
            .body
        );
        assert_eq!(
            run(
                config(Some("new system"), Some("new file")),
                route,
                format,
                &empty,
                preview.body.clone(),
            )
            .body,
            preview.body,
        );
    }
}

#[test]
fn empty_source_injection_requires_ready_absolute_source_and_active_nonempty_target() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let empty = Source {
            content: Some(String::new()),
            ..source(route)
        };
        let (body, _) = native_system(route.wire_format(), "system");
        for invalid in [
            Source::default(),
            Source {
                state: SourceState::Missing,
                ..empty.clone()
            },
            Source {
                state: SourceState::Error,
                ..empty.clone()
            },
            Source {
                content: None,
                ..empty.clone()
            },
            Source {
                path: None,
                ..empty.clone()
            },
            Source {
                path: Some("relative/CLAUDE.md".into()),
                ..empty.clone()
            },
            Source {
                path: Some("/home/me/../CLAUDE.md".into()),
                ..empty.clone()
            },
        ] {
            let preview = run(
                config(None, Some("must not inject")),
                route,
                route.wire_format(),
                &invalid,
                body.clone(),
            );
            assert!(!preview.user_applied);
            assert_eq!(preview.body, body);
        }
        // Any loaded source at a real absolute path injects when the envelope is absent.
        for loaded in [
            Source {
                content: Some(" \n".into()),
                ..empty.clone()
            },
            source(route),
        ] {
            let preview = run(
                config(None, Some("new file")),
                route,
                route.wire_format(),
                &loaded,
                body.clone(),
            );
            assert_eq!(preview.file_status, FileStatus::Injected);
            assert!(preview.user_applied);
            assert!(preview
                .body
                .to_string()
                .contains("<cc-switch-user-instructions>"));
        }
        for target in [None, Some("")] {
            let preview = run(
                config(None, target),
                route,
                route.wire_format(),
                &empty,
                body.clone(),
            );
            assert!(!preview.user_applied);
            assert_eq!(preview.body, body);
        }
        for disabled in 0..4 {
            let mut cfg = config(None, Some("must not inject"));
            if disabled == 0 {
                cfg.enabled = false;
            }
            let route_cfg = match route {
                Route::ClaudeCode => &mut cfg.routes.claude_code,
                Route::ClaudeDesktop => &mut cfg.routes.claude_desktop,
            };
            match disabled {
                1 => route_cfg.enabled = false,
                2 => route_cfg.user_instructions.enabled = false,
                3 => route_cfg.user_instructions.replacement = None,
                _ => {}
            }
            let preview = run(cfg, route, route.wire_format(), &empty, body.clone());
            assert_eq!(preview.body, body);
            assert!(!preview.user_applied);
        }
    }
}

#[test]
fn empty_source_existing_stale_duplicate_and_partial_envelopes_never_inject() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let empty = Source {
            content: Some(String::new()),
            ..source(route)
        };
        let wrapper = envelope(route, &empty);
        let field = "messages";
        let (mut body, _) = native_system(route.wire_format(), "system");
        body[field]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"role":"user","content":wrapper}));
        let preview = run(
            config(Some("new system"), Some("new file")),
            route,
            route.wire_format(),
            &empty,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.user_applied && preview.system_applied);
        assert_eq!(preview.body[field].as_array().unwrap().len(), 2);
        assert!(preview.body[field][0]["content"]
            .as_str()
            .unwrap()
            .contains("new file"));
        // An envelope carrying an older copy of this file is replaced in place, not duplicated.
        body[field][0]["content"] = json!(envelope(route, &source(route)));
        let preview = run(
            config(None, Some("new file")),
            route,
            route.wire_format(),
            &empty,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.user_applied);
        let rewritten = preview.body[field][0]["content"].as_str().unwrap();
        assert!(rewritten.contains("new file"));
        assert!(!rewritten.contains("Original global instructions."));

        // Repeated copies of the same file envelope are all replaced, never injected twice.
        body[field][0]["content"] = json!(format!("{wrapper}\n{wrapper}"));
        let preview = run(
            config(None, Some("new file")),
            route,
            route.wire_format(),
            &empty,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Identified);
        assert!(preview.user_applied);
        assert_eq!(
            preview.body[field][0]["content"]
                .as_str()
                .unwrap()
                .matches("new file")
                .count(),
            2
        );

        // Truncated or prose-wrapped carriers are still never guessed at and never duplicated.
        for text in [
            wrapper.replace("</", "<broken/"),
            format!("{wrapper}\nordinary chat"),
        ] {
            body[field][0]["content"] = json!(text);
            let preview = run(
                config(None, Some("must not inject")),
                route,
                route.wire_format(),
                &empty,
                body.clone(),
            );
            assert!(!preview.user_applied);
            assert_eq!(preview.body, body);
        }
    }
}

#[test]
fn empty_source_preserves_project_envelopes_and_supports_custom_paths() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let mut empty = Source {
            content: Some(String::new()),
            ..source(route)
        };
        empty.path = Some("/home/me/custom/CLAUDE.md".into());
        let project = Source {
            path: Some("/project/CLAUDE.md".into()),
            content: Some("project kept".into()),
            ..empty.clone()
        };
        let field = "messages";
        let (mut body, _) = native_system(route.wire_format(), "system");
        body[field].as_array_mut().unwrap().insert(
            0,
            json!({"role":"user","content":envelope(route, &project)}),
        );
        let preview = run(
            config(None, Some("new file")),
            route,
            route.wire_format(),
            &empty,
            body.clone(),
        );
        assert_eq!(preview.file_status, FileStatus::Injected);
        let mut actual = preview.body;
        actual[field].as_array_mut().unwrap().remove(0);
        assert_eq!(actual, body);
    }
}

#[test]
fn empty_source_responses_string_and_history_reference_are_preserved() {
    let route = Route::ClaudeCode;
    let empty = Source {
        content: Some(String::new()),
        ..source(route)
    };
    for input in [json!("unaltered chat"), json!([]), Value::Null] {
        let body = json!({"instructions":"system","input":input,"previous_response_id":"resp_previous","model":"client"});
        let preview = run(
            config(Some("new system"), Some("new file")),
            route,
            WireFormat::Responses,
            &empty,
            body,
        );
        assert!(preview.system_applied && preview.user_applied);
        assert_eq!(preview.body["instructions"], "new system");
        assert_eq!(preview.body["previous_response_id"], "resp_previous");
        assert_eq!(preview.body["model"], "client");
        let messages = preview.body["input"].as_array().unwrap();
        assert_eq!(messages.len(), if input.is_string() { 2 } else { 1 });
        if input.is_string() {
            assert_eq!(messages[1], json!({"role":"user","content":input}));
        }
    }
}

#[test]
fn empty_source_uncertain_shared_boundary_and_invalid_container_are_atomic() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let empty = Source {
            content: Some(String::new()),
            ..source(route)
        };
        let (mut body, _) = native_system(route.wire_format(), "system");
        let field = "messages";
        body[field] = json!({"unsupported":true});
        let original = body.clone();
        let ctx = RewriteContext::for_test_with_source(
            config(Some("new system"), Some("new file")),
            route,
            empty.clone(),
        )
        .unwrap();
        assert!(ctx.rewrite_ingress(&mut body).is_err());
        assert_eq!(body, original);
        for text in [
            "system\n<system-reminder>\npartial",
            "system\n</system-reminder>",
        ] {
            let (body, _) = native_system(route.wire_format(), text);
            let preview = run(
                config(Some("new system"), Some("new file")),
                route,
                route.wire_format(),
                &empty,
                body.clone(),
            );
            assert_eq!(preview.body, body);
            assert!(!preview.user_applied && !preview.system_applied);
        }
    }
}

#[test]
fn empty_source_does_not_duplicate_uneditable_leading_file_carriers() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let empty = Source {
            content: Some(String::new()),
            ..source(route)
        };
        let field = "messages";
        let kind = "text";
        for wrapper in [
            envelope(route, &source(route)),
            desktop_global_envelope(&source(route)),
        ] {
            for extra in [
                json!({"type":kind,"text":"ordinary chat"}),
                json!({"type":"image","source":{"type":"base64","data":"opaque"}}),
            ] {
                for blocks in [
                    json!([{"type":kind,"text":wrapper},extra]),
                    json!([extra,{"type":kind,"text":wrapper}]),
                ] {
                    let body = json!({field:[{"role":"user","content":blocks}]});
                    let preview = run(
                        config(None, Some("must not inject")),
                        route,
                        route.wire_format(),
                        &empty,
                        body.clone(),
                    );
                    // Only the verified Desktop first-block form is a carrier next to other
                    // blocks. It is replaced in place and never duplicated as an extra message.
                    let replaced = wrapper.contains("Codebase and user instructions")
                        && extra["type"] == "text"
                        && blocks[0]["text"].as_str() == Some(wrapper.as_str());
                    if replaced {
                        assert!(preview.user_applied);
                        assert_eq!(preview.body[field].as_array().unwrap().len(), 1);
                        assert_ne!(preview.body, body);
                    } else {
                        assert_eq!(preview.body, body);
                        assert!(!preview.user_applied);
                    }
                }
            }
        }
        {
            let body = json!({"input":envelope(route, &source(route))});
            let preview = run(
                config(None, Some("must not inject")),
                route,
                WireFormat::Responses,
                &empty,
                body.clone(),
            );
            assert_eq!(preview.body, body);
            assert!(!preview.user_applied);
        }
    }
}

#[test]
fn empty_source_checks_noneditable_blocks_after_desktop_project_prefix() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let empty = Source {
            content: Some(String::new()),
            ..source(route)
        };
        let project = Source {
            path: Some("/project/CLAUDE.md".into()),
            content: Some("project instructions".into()),
            ..source(route)
        };
        for stale in [
            envelope(route, &source(route)),
            desktop_global_envelope(&source(route)),
        ] {
            let body = json!({"messages":[{"role":"user","content":[
                {"type":"text","text":desktop_global_envelope(&project)},
                {"type":"text","text":"ordinary context"},
                {"type":"text","text":stale}
            ]}]});
            let preview = run(
                config(None, Some("must not inject")),
                route,
                route.wire_format(),
                &empty,
                body.clone(),
            );
            assert_eq!(preview.body, body);
            assert!(!preview.user_applied);
        }
    }
}

#[test]
fn empty_source_read_and_injection_never_write_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CLAUDE.md");
    std::fs::write(&path, b"").unwrap();
    let source = sources::read_source(&path);
    let (body, _) = native_system(WireFormat::Responses, "system");
    let preview = run(
        config(Some("new system"), Some("new file")),
        Route::ClaudeCode,
        WireFormat::Responses,
        &source,
        body,
    );
    assert_eq!(preview.file_status, FileStatus::Injected);
    assert_eq!(std::fs::read(&path).unwrap(), b"");
}

#[test]
fn atomic_edit_plan_error_retains_actual_ingress_request() {
    for (route, mut body) in [
        (Route::ClaudeDesktop, json!({"system":{"unsupported":true}})),
        (Route::ClaudeCode, json!({"system":{"unsupported":true}})),
    ] {
        let context = RewriteContext::for_test(config(Some("new"), None), route).unwrap();
        let original = body.clone();
        assert!(context.rewrite_ingress(&mut body).is_err());
        assert_eq!(body, original);
    }
}

#[test]
fn context_freezes_source_and_config_for_one_ingress_rewrite() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let mut source = source(route);
        let wrapper = envelope(route, &source);
        let mut configuration = config(Some("system"), Some("replacement"));
        let context =
            RewriteContext::for_test_with_source(configuration.clone(), route, source.clone())
                .unwrap();
        source.content = Some("later source revision".into());
        configuration.enabled = false;
        let (mut body, path) = native_system(route.wire_format(), &format!("old\n{wrapper}"));
        body["model"] = json!("original-client-model");
        context.rewrite_ingress(&mut body).unwrap();
        assert_eq!(
            body.pointer(path).unwrap(),
            &json!(format!(
                "system\n{}",
                wrapper.replace("Original global instructions.\n第二行", "replacement")
            ))
        );
        assert_eq!(body["model"], "original-client-model");
    }
}

#[test]
fn source_reader_distinguishes_empty_missing_invalid_utf8_directory_and_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("CLAUDE.md");
    let missing = sources::read_source(&path);
    assert_eq!(missing.state, SourceState::Missing);
    assert_eq!(missing.content, None);
    assert!(missing.loaded_at.is_some());
    std::fs::write(&path, b"").unwrap();
    let empty = sources::read_source(&path);
    assert_eq!(empty.state, SourceState::Ready);
    assert_eq!(empty.content, Some(String::new()));
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    let invalid = sources::read_source(&path);
    assert_eq!(invalid.state, SourceState::Error);
    assert!(invalid.error.unwrap().contains("UTF-8"));
    std::fs::write(&path, vec![b'x'; config::MAX_TEXT_BYTES + 1]).unwrap();
    assert!(sources::read_source(&path)
        .error
        .unwrap()
        .contains("256 KiB"));
    assert_eq!(sources::read_source(dir.path()).state, SourceState::Error);
}

#[test]
fn v1_and_malformed_configs_are_explicitly_rejected_without_overwriting_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    for bytes in [
        b"{\"version\":1,\"enabled\":true,\"routes\":{},\"rules\":[]}".as_slice(),
        b"{secret-invalid-json".as_slice(),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let status = PromptRewriter::load_at(path.clone()).status().unwrap();
        assert_eq!(status.config, Config::default());
        assert!(status.error.is_some());
        assert!(!status.error.as_deref().unwrap().contains("secret"));
        if bytes.starts_with(b"{\"version\":1") {
            assert!(status.error.as_deref().unwrap().contains("v1"));
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, vec![b' '; MAX_CONFIG_BYTES + 1]).unwrap();
    assert!(PromptRewriter::load_at(path)
        .status()
        .unwrap()
        .error
        .unwrap()
        .contains("1 MiB"));
}

#[test]
fn config_io_source_refresh_and_failure_recovery_leave_official_and_instruction_hashes_unchanged() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("extensions/prompt-rewriter.json");
    let files = [
        "CLAUDE.md",
        "AGENTS.md",
        "AGENTS.override.md",
        "custom.md",
        "config.json",
        "settings.json",
        "cc-switch.db",
        "cc-switch.db-wal",
        "extensions/other.json",
    ];
    for name in files {
        let file = root.join(name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, format!("sentinel-{name}\r\n")).unwrap();
    }
    let hashes: Vec<_> = files
        .iter()
        .map(|name| Sha256::digest(std::fs::read(root.join(name)).unwrap()))
        .collect();
    let assert_unchanged = || {
        for (name, hash) in files.iter().zip(&hashes) {
            assert_eq!(
                Sha256::digest(std::fs::read(root.join(name)).unwrap()),
                *hash,
                "{name}"
            );
        }
    };
    let runtime = PromptRewriter::load_at(path.clone());
    assert_unchanged();
    let mut draft = config(Some("system"), Some("file"));
    draft.routes.claude_code.user_instructions.source_path =
        Some(root.join("CLAUDE.md").to_string_lossy().into_owned());
    draft.routes.claude_desktop.user_instructions.source_path =
        Some(root.join("custom.md").to_string_lossy().into_owned());
    runtime
        .save_with_path(draft.clone(), || path.clone())
        .unwrap();
    assert_unchanged();
    let status = runtime.reload_with_path(|| path.clone()).unwrap();
    assert_eq!(status.config, draft);
    assert_eq!(status.sources.claude_code.state, SourceState::Ready);
    assert_unchanged();
    runtime.refresh_with_path(|| path.clone()).unwrap();
    assert_unchanged();
    std::fs::write(&path, "broken extension config").unwrap();
    assert!(runtime
        .reload_with_path(|| path.clone())
        .unwrap()
        .error
        .is_some());
    assert_unchanged();
    assert_eq!(
        runtime.refresh_with_path(|| path.clone()).unwrap().config,
        draft
    );
    assert_unchanged();
    runtime
        .save_with_path(Config::default(), || path.clone())
        .unwrap();
    assert_unchanged();
}

#[test]
fn source_cannot_alias_the_extension_config_write_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    runtime
        .save_with_path(Config::default(), || path.clone())
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let mut draft = config(Some("new"), Some("file"));
        let target = match route {
            Route::ClaudeDesktop => &mut draft.routes.claude_desktop,
            Route::ClaudeCode => &mut draft.routes.claude_code,
        };
        target.user_instructions.source_path = Some(path.to_string_lossy().into_owned());
        assert!(runtime
            .save_with_path(draft, || path.clone())
            .unwrap_err()
            .contains("aliases"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(runtime.status().unwrap().config, Config::default());
    }
}

#[test]
fn source_cache_is_refreshed_explicitly_without_reloading_or_clobbering_memory_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("extensions/prompt-rewriter.json");
    let source_path = dir.path().join("CLAUDE.md");
    std::fs::write(&source_path, "first source").unwrap();
    let mut draft = config(Some("saved system"), Some("saved file"));
    draft.routes.claude_code.user_instructions.source_path =
        Some(source_path.to_string_lossy().into_owned());
    let runtime = PromptRewriter::load_at(path.clone());
    runtime
        .save_with_path(draft.clone(), || path.clone())
        .unwrap();
    let context = runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .unwrap();
    std::fs::write(&source_path, "external edit").unwrap();
    std::fs::write(&path, "invalid on-disk extension JSON").unwrap();
    assert_eq!(
        runtime
            .status()
            .unwrap()
            .sources
            .claude_code
            .content
            .as_deref(),
        Some("first source")
    );
    assert_eq!(context.source.content.as_deref(), Some("first source"));
    let status = runtime.refresh_with_path(|| path.clone()).unwrap();
    assert_eq!(status.config, draft);
    assert_eq!(
        status.sources.claude_code.content.as_deref(),
        Some("external edit")
    );
    assert_eq!(context.source.content.as_deref(), Some("first source"));
    std::fs::remove_file(&source_path).unwrap();
    assert_eq!(
        runtime
            .refresh_with_path(|| path.clone())
            .unwrap()
            .sources
            .claude_code
            .state,
        SourceState::Missing
    );
}

#[test]
fn same_path_reload_failure_retains_revision_but_directory_switch_clears_sources_and_observations()
{
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.json");
    let other = dir.path().join("new.json");
    let runtime = PromptRewriter::load_at(path.clone());
    let draft = config(Some("saved"), None);
    runtime
        .save_with_path(draft.clone(), || path.clone())
        .unwrap();
    runtime
        .arm_with_path(Route::ClaudeCode, path.clone())
        .unwrap();
    let context = runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .unwrap();
    std::fs::write(&path, "bad").unwrap();
    let status = runtime.reload_with_path(|| path.clone()).unwrap();
    assert_eq!(status.config, draft);
    assert!(status.error.is_some());
    assert_eq!(
        status.observations.claude_code.state,
        ObservationState::Waiting
    );
    assert!(runtime
        .save_with_path(Config::default(), || other.clone())
        .is_err());
    assert!(!other.exists());
    std::fs::write(&other, "bad").unwrap();
    let status = runtime.reload_with_path(|| other.clone()).unwrap();
    assert_eq!(status.config, Config::default());
    assert_eq!(status.sources.claude_code, Source::default());
    assert_eq!(
        status.observations.claude_code.state,
        ObservationState::NotCaptured
    );
    context
        .rewrite_ingress(&mut json!({"system":"old request"}))
        .unwrap();
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.state,
        ObservationState::NotCaptured
    );
}

#[test]
fn stale_request_path_cannot_discard_reloaded_config() {
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.json");
        let current = dir.path().join("current.json");
        let changed = dir.path().join("changed.json");
        let draft = config(Some("current system"), Some("current user"));
        std::fs::write(&current, serde_json::to_vec(&draft).unwrap()).unwrap();
        let runtime = PromptRewriter::load_at(old.clone());
        runtime.reload_with_path(|| current.clone()).unwrap();
        runtime
            .arm_with_current_path(route, || {
                assert!(runtime.updates.try_lock().is_err());
                current.clone()
            })
            .unwrap();
        let before = runtime.status().unwrap();
        let calls = std::cell::Cell::new(0);
        // The first read is stale; the second resolves the directory after the reload.
        assert!(runtime
            .context_with_path(route, || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    old.clone()
                } else {
                    assert!(runtime.updates.try_lock().is_err());
                    current.clone()
                }
            })
            .unwrap()
            .is_none());
        assert_eq!(calls.get(), 2);
        assert_eq!(
            serde_json::to_value(runtime.status().unwrap()).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert!(runtime
            .context_with_path(route, || current.clone())
            .unwrap()
            .is_some());
        // A genuine directory change without reload still disables the old snapshot.
        assert!(runtime
            .context_with_path(route, || changed.clone())
            .unwrap()
            .is_none());
        let status = runtime.status().unwrap();
        assert_eq!(status.config, Config::default());
        assert_eq!(status.path, changed.to_string_lossy());
        assert!(status.error.is_some());
    }
}

#[test]
fn invalid_save_atomic_io_failure_and_path_changes_retain_valid_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.json");
    let other = dir.path().join("other.json");
    let runtime = PromptRewriter::load_at(path.clone());
    let draft = config(Some("old"), None);
    runtime
        .save_with_path(draft.clone(), || path.clone())
        .unwrap();
    let disk = std::fs::read(&path).unwrap();
    let mut invalid = draft.clone();
    invalid.version = 1;
    assert!(runtime.save_with_path(invalid, || path.clone()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), disk);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(runtime
        .save_with_path(Config::default(), || path.clone())
        .is_err());
    assert_eq!(runtime.status().unwrap().config, draft);
    let calls = Mutex::new(0);
    assert!(runtime
        .reload_with_path(|| {
            let mut count = calls.lock().unwrap();
            *count += 1;
            if *count == 1 {
                path.clone()
            } else {
                other.clone()
            }
        })
        .is_err());
    assert_eq!(runtime.status().unwrap().config, Config::default());
}

#[test]
fn concurrent_config_updates_publish_only_complete_revisions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = Arc::new(PromptRewriter::load_at(path.clone()));
    let barrier = Arc::new(Barrier::new(6));
    std::thread::scope(|scope| {
        for index in 0..6 {
            let runtime = Arc::clone(&runtime);
            let barrier = Arc::clone(&barrier);
            let path = path.clone();
            scope.spawn(move || {
                barrier.wait();
                runtime
                    .save_with_path(config(Some(&index.to_string()), None), || path.clone())
                    .unwrap();
            });
        }
    });
    let disk: Config = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(runtime.status().unwrap().config, disk);
    assert_eq!(
        disk.routes.claude_code.system.replacement,
        disk.routes.claude_desktop.system.replacement
    );
}

#[test]
fn capture_is_opt_in_route_local_and_works_while_rewriting_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    assert!(runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .is_none());
    runtime
        .arm_with_path(Route::ClaudeCode, path.clone())
        .unwrap();
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.state,
        ObservationState::Waiting
    );
    assert!(runtime
        .context_at(Route::ClaudeDesktop, path.clone())
        .unwrap()
        .is_none());
    let context = runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .unwrap();
    let body = json!({"system":"original client instructions","model":"test-model","messages":[{"role":"user","content":"CHAT HISTORY MUST NOT BE CAPTURED"}],"api_key":"secret"});
    let mut actual = body.clone();
    context.rewrite_ingress(&mut actual).unwrap();
    assert_eq!(actual, body);
    let status = runtime.status().unwrap();
    let captured = status.observations.claude_code;
    assert_eq!(captured.state, ObservationState::Captured);
    assert_eq!(
        captured.system.as_deref(),
        Some("original client instructions")
    );
    assert_eq!(captured.model.as_deref(), Some("test-model"));
    assert_eq!(captured.provider_id, None);
    assert_eq!(captured.format, Some(WireFormat::Anthropic));
    assert!(captured.captured_at.unwrap() > 1_000_000_000_000);
    assert!(captured.request_id.is_some());
    assert!(!captured.system_applied && !captured.user_applied);
    assert!(!path.exists());
    let serialized = serde_json::to_string(&captured).unwrap();
    assert!(!serialized.contains("CHAT HISTORY"));
    assert!(!serialized.contains("secret"));
    assert!(runtime
        .context_at(Route::ClaudeCode, path)
        .unwrap()
        .is_none());
}

#[test]
fn claude_routes_have_independent_settings_sources_and_capture_slots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    let mut configuration = config(Some("system"), Some("file"));
    let routes = [Route::ClaudeDesktop, Route::ClaudeCode];
    for (index, route) in routes.into_iter().enumerate() {
        let source_path = dir.path().join(format!("route-{index}")).join("CLAUDE.md");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, format!("source-{index}")).unwrap();
        let target = match route {
            Route::ClaudeDesktop => &mut configuration.routes.claude_desktop,
            Route::ClaudeCode => &mut configuration.routes.claude_code,
        };
        target.system.replacement = Some(format!("system-{index}"));
        target.user_instructions.replacement = Some(format!("file-{index}"));
        target.user_instructions.source_path =
            Some(source_path.to_string_lossy().replace('\\', "/"));
    }
    runtime
        .save_with_path(configuration.clone(), || path.clone())
        .unwrap();
    for route in routes {
        runtime.arm_with_path(route, path.clone()).unwrap();
    }
    for (index, route) in routes.into_iter().enumerate() {
        let context = runtime.context_at(route, path.clone()).unwrap().unwrap();
        assert_eq!(
            context.source.content.as_deref(),
            Some(format!("source-{index}").as_str())
        );
        let wrapper = envelope(route, &context.source);
        let (mut body, system_path) =
            native_system(route.wire_format(), &format!("original-{index}\n{wrapper}"));
        body["model"] = json!(format!("client-{index}"));
        context.rewrite_ingress(&mut body).unwrap();
        let rewritten = body.pointer(system_path).unwrap().as_str().unwrap();
        assert!(rewritten.starts_with(&format!("system-{index}\n")));
        assert!(rewritten.contains(&format!("file-{index}")));
        assert!(!rewritten.contains(&format!("source-{index}")));
        let mut status = runtime.status().unwrap();
        for (other_index, other_route) in routes.into_iter().enumerate() {
            let observed = status.observations.get_mut(other_route);
            if other_index > index {
                assert_eq!(observed.state, ObservationState::Waiting);
            } else {
                assert_eq!(observed.state, ObservationState::Captured);
                assert_eq!(
                    observed.model.as_deref(),
                    Some(format!("client-{other_index}").as_str())
                );
                assert_eq!(observed.provider_id, None);
                assert!(observed.system_applied && observed.user_applied);
            }
        }
    }
    // Desktop and Code share an ingress format, never a route enable switch or replacement.
    for enabled_route in routes {
        let mut single = configuration.clone();
        single.routes.claude_desktop.enabled = enabled_route == Route::ClaudeDesktop;
        single.routes.claude_code.enabled = enabled_route == Route::ClaudeCode;
        runtime.save_with_path(single, || path.clone()).unwrap();
        for route in routes {
            assert_eq!(
                runtime.context_at(route, path.clone()).unwrap().is_some(),
                route == enabled_route
            );
        }
    }
}

#[test]
fn capture_ownership_is_one_shot_and_rearm_rejects_stale_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    runtime
        .arm_with_path(Route::ClaudeCode, path.clone())
        .unwrap();
    let first = runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .unwrap();
    let concurrent = runtime
        .context_at(Route::ClaudeCode, path.clone())
        .unwrap()
        .unwrap();
    first
        .rewrite_ingress(&mut json!({"system":"original ingress","model":"client-model"}))
        .unwrap();
    let original = runtime.status().unwrap().observations.claude_code;
    concurrent
        .rewrite_ingress(&mut json!({"system":"other request","model":"other-model"}))
        .unwrap();
    assert_eq!(runtime.status().unwrap().observations.claude_code, original);
    first
        .rewrite_ingress(&mut json!({"system":"later body","model":"mapped-model"}))
        .unwrap();
    assert_eq!(runtime.status().unwrap().observations.claude_code, original);
    assert_eq!(original.system.as_deref(), Some("original ingress"));
    assert_eq!(original.model.as_deref(), Some("client-model"));
    assert_eq!(original.provider_id, None);
    runtime
        .arm_with_path(Route::ClaudeCode, path.clone())
        .unwrap();
    first
        .rewrite_ingress(&mut json!({"system":"stale context"}))
        .unwrap();
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.state,
        ObservationState::Waiting
    );
    runtime
        .context_at(Route::ClaudeCode, path)
        .unwrap()
        .unwrap()
        .rewrite_ingress(&mut json!({}))
        .unwrap();
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.state,
        ObservationState::Captured
    );
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.system,
        None
    );
}

#[test]
fn capture_is_before_rewrite_bounds_memory_and_distinguishes_empty_from_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    runtime
        .save_with_path(config(Some("rewritten"), None), || path.clone())
        .unwrap();
    for text in [String::new(), "x".repeat(config::MAX_TEXT_BYTES + 1)] {
        runtime
            .arm_with_path(Route::ClaudeCode, path.clone())
            .unwrap();
        let context = runtime
            .context_at(Route::ClaudeCode, path.clone())
            .unwrap()
            .unwrap();
        let mut body = json!({"system":text});
        context.rewrite_ingress(&mut body).unwrap();
        assert_eq!(body["system"], "rewritten");
        let observed = runtime.status().unwrap().observations.claude_code;
        if text.is_empty() {
            assert_eq!(observed.state, ObservationState::Captured);
            assert_eq!(observed.system, Some(String::new()));
        } else {
            assert_eq!(observed.state, ObservationState::Error);
            assert_eq!(observed.system, None);
            assert!(observed.error.is_some());
        }
        assert!(observed.system_applied);
    }
    let disk = std::fs::read_to_string(path).unwrap();
    assert!(!disk.contains("capturedAt"));
    assert!(!disk.contains("providerId"));
}

#[cfg(feature = "prompt-rewriter")]
#[test]
fn preview_accepts_only_each_routes_ingress_format() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = PromptRewriter::load_at(dir.path().join("prompt-rewriter.json"));
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        for format in FORMATS {
            let (body, _) = native_system(format, "original");
            let result = runtime.preview(config(Some("replacement"), None), route, format, body);
            if format == route.wire_format() {
                assert!(result.unwrap().system_applied);
            } else {
                assert!(result.unwrap_err().contains("ingress wire format"));
            }
        }
    }
}

#[cfg(feature = "prompt-rewriter")]
#[test]
fn preview_is_pure_does_not_arm_capture_load_draft_paths_or_change_memory_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    runtime
        .arm_with_path(Route::ClaudeCode, path.clone())
        .unwrap();
    let draft = config(Some("draft"), None);
    let preview = runtime
        .preview(
            draft,
            Route::ClaudeCode,
            WireFormat::Anthropic,
            json!({"system":"current"}),
        )
        .unwrap();
    assert_eq!(preview.system_before.as_deref(), Some("current"));
    assert_eq!(preview.system_after.as_deref(), Some("draft"));
    assert!(preview.system_applied);
    assert_eq!(runtime.status().unwrap().config, Config::default());
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.state,
        ObservationState::Waiting
    );
    assert!(!path.exists());
    let mut disabled = config(Some("draft"), None);
    disabled.enabled = false;
    disabled.routes.claude_code.enabled = false;
    assert!(
        runtime
            .preview(
                disabled.clone(),
                Route::ClaudeCode,
                WireFormat::Anthropic,
                json!({})
            )
            .unwrap()
            .system_applied
    );
    disabled.routes.claude_code.system.enabled = false;
    assert!(
        !runtime
            .preview(
                disabled,
                Route::ClaudeCode,
                WireFormat::Anthropic,
                json!({})
            )
            .unwrap()
            .system_applied
    );
    assert_eq!(runtime.status().unwrap().config, Config::default());
    assert_eq!(
        runtime.status().unwrap().observations.claude_code.state,
        ObservationState::Waiting
    );
}

#[test]
fn begin_request_skip_or_absent_app_are_noops_and_update_guard_tracks_build_feature() {
    for skip in [true, false] {
        assert!(begin_request(None, Route::ClaudeCode, skip)
            .unwrap()
            .is_none());
    }
    assert_eq!(
        ensure_custom_update_allowed().is_err(),
        cfg!(feature = "prompt-rewriter")
    );
    let context = RewriteContext::for_test(Config::default(), Route::ClaudeCode).unwrap();
    assert_eq!(context.source, Source::default());
}

#[cfg(unix)]
#[test]
fn unix_source_and_config_fifos_are_rejected_without_waiting_for_writers() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let source_fifo = directory.path().join("instruction.fifo");
    let config_fifo = directory.path().join("config.fifo");
    for path in [&source_fifo, &config_fifo] {
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a NUL-terminated path owned for this call; mkfifo retains no pointer.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    }
    let source_link = directory.path().join("instruction-link");
    std::os::unix::fs::symlink(&source_fifo, &source_link).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let source = sources::read_source(&source_fifo);
        let linked_source = sources::read_source(&source_link);
        let config_error = read_snapshot(&config_fifo).err();
        sender.send((source, linked_source, config_error)).unwrap();
    });
    let (source, linked_source, config_error) = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("source/config FIFO reads must return without a writer");
    worker.join().unwrap();
    assert_eq!(source.state, SourceState::Error);
    assert_eq!(linked_source.state, SourceState::Error);
    assert_eq!(source.content, None);
    assert!(config_error.unwrap().contains("cannot open config"));
}

#[test]
fn capture_uses_only_original_client_model_and_bounds_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    for route in [Route::ClaudeDesktop, Route::ClaudeCode] {
        for body_model in [None, Some("client-model"), Some("")] {
            runtime.arm_with_path(route, path.clone()).unwrap();
            let context = runtime.context_at(route, path.clone()).unwrap().unwrap();
            let (mut body, _) = native_system(route.wire_format(), "actual ingress system");
            if let Some(model) = body_model {
                body["model"] = json!(model);
            }
            let context = context.with_client_model(&body);
            body["model"] = json!("stack-resolved-upstream-model");
            let original = body.clone();
            context.rewrite_ingress(&mut body).unwrap();
            assert_eq!(body, original);
            let mut status = runtime.status().unwrap();
            let captured = status.observations.get_mut(route);
            assert_eq!(captured.state, ObservationState::Captured);
            assert_eq!(captured.model.as_deref(), body_model);
            assert_eq!(captured.provider_id, None);
            assert_eq!(captured.format, Some(route.wire_format()));
            assert_eq!(captured.system.as_deref(), Some("actual ingress system"));
        }
        runtime.arm_with_path(route, path.clone()).unwrap();
        let context = runtime.context_at(route, path.clone()).unwrap().unwrap();
        let (mut body, _) = native_system(route.wire_format(), "system");
        body["model"] = json!("x".repeat(1025));
        let original = body.clone();
        context.rewrite_ingress(&mut body).unwrap();
        assert_eq!(body, original);
        let mut status = runtime.status().unwrap();
        let captured = status.observations.get_mut(route);
        assert_eq!(captured.state, ObservationState::Error);
        assert_eq!(captured.model, None);
        assert_eq!(captured.provider_id, None);
        assert!(captured
            .error
            .as_deref()
            .unwrap()
            .contains("metadata exceeds 1024 bytes"));
    }
}

#[test]
fn runtime_with_disabled_or_null_targets_preserves_oversize_request() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("prompt-rewriter.json");
    let runtime = PromptRewriter::load_at(path.clone());
    let disabled = config(None, None);
    let mut null = disabled.clone();
    null.routes.claude_code.system.enabled = true;
    null.routes.claude_code.user_instructions.enabled = true;
    let mut inactive_values = config(Some("system"), Some("file"));
    inactive_values.routes.claude_code.system.enabled = false;
    inactive_values.routes.claude_code.user_instructions.enabled = false;
    for configuration in [disabled, null, inactive_values] {
        runtime
            .save_with_path(configuration, || path.clone())
            .unwrap();
        for capture in [false, true] {
            if capture {
                runtime
                    .arm_with_path(Route::ClaudeCode, path.clone())
                    .unwrap();
            }
            let context = runtime
                .context_at(Route::ClaudeCode, path.clone())
                .unwrap()
                .unwrap();
            let mut body = json!({"system":"x".repeat(8 * 1024 * 1024 + 1),"messages":[{"role":"user","content":"unchanged chat"}]});
            let original = body.clone();
            context.rewrite_ingress(&mut body).unwrap();
            assert_eq!(body, original);
            if capture {
                let observed = runtime.status().unwrap().observations.claude_code;
                assert_eq!(observed.state, ObservationState::Error);
                assert!(!observed.system_applied && !observed.user_applied);
            }
        }
    }
}

#[cfg(not(feature = "prompt-rewriter"))]
#[test]
fn unfeatured_commands_do_not_read_sources_or_write_extension_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("prompt-rewriter.json");
    let runtime =
        PromptRewriter::from_state(path.clone(), default_snapshot(), Sources::default(), None);
    assert!(!runtime.status().unwrap().available);
    assert!(runtime.save(Config::default()).is_err());
    assert!(runtime.reload().is_err());
    assert!(runtime.refresh_sources().is_err());
    assert!(runtime.capture(Route::ClaudeCode).is_err());
    assert!(runtime
        .preview(
            Config::default(),
            Route::ClaudeCode,
            WireFormat::Anthropic,
            json!({})
        )
        .is_err());
    assert!(!path.exists());
}
