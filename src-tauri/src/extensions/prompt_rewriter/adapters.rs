//! Native text carriers only. Tool payloads, media, assistant text and ordinary chat are opaque.
use super::WireFormat;
use serde_json::{json, Value};

pub(crate) struct TextNode {
    pub path: String,
    pub remove_path: String,
    pub text: String,
    pub system: bool,
    pub message: Option<String>,
}

#[derive(Default)]
pub(crate) struct Selection {
    pub nodes: Vec<TextNode>,
    // Read-only candidates in mixed/media messages or string input: never editable, but
    // an existing file envelope here must still prevent empty-source duplicate injection.
    pub rejected_file_carriers: Vec<String>,
}

pub(super) const CLAUDE_GLOBAL_PREFIX: &str = "<system-reminder>\nCodebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.\n\nContents of ";

pub(crate) fn envelope_hint(text: &str) -> bool {
    text.starts_with("<system-reminder>\n")
}

fn string_node(
    body: &Value,
    path: &str,
    remove_path: &str,
    system: bool,
    nodes: &mut Vec<TextNode>,
) {
    if let Some(text) = body.pointer(path).and_then(Value::as_str) {
        nodes.push(TextNode {
            path: path.into(),
            remove_path: remove_path.into(),
            text: text.into(),
            system,
            message: None,
        });
    }
}

fn content_nodes(body: &Value, path: &str, format: WireFormat, system: bool) -> Vec<TextNode> {
    let mut nodes = Vec::new();
    match body.pointer(path) {
        Some(Value::String(_)) => string_node(body, path, path, system, &mut nodes),
        Some(Value::Array(blocks)) => {
            for (index, block) in blocks.iter().enumerate() {
                let recognized = match format {
                    WireFormat::Anthropic | WireFormat::Chat => block["type"] == "text",
                    WireFormat::Responses => {
                        block["type"] == "input_text" || block["type"] == "text"
                    }
                    WireFormat::Gemini => block.as_object().is_some_and(|object| {
                        object.contains_key("text")
                            && object.keys().all(|key| {
                                key == "text" || key == "thought" || key == "thoughtSignature"
                            })
                            && object.get("thought").and_then(Value::as_bool) != Some(true)
                            && !object.contains_key("thoughtSignature")
                    }),
                };
                if recognized {
                    let remove = format!("{path}/{index}");
                    string_node(body, &format!("{remove}/text"), &remove, system, &mut nodes);
                }
            }
        }
        _ => {}
    }
    nodes
}

fn is_message(value: &Value) -> bool {
    value.is_object() && (value.get("type").is_none() || value["type"] == "message")
}

fn messages(
    body: &Value,
    field: &str,
    format: WireFormat,
    nodes: &mut Vec<TextNode>,
    rejected_file_carriers: &mut Vec<String>,
) {
    let Some(messages) = body[field].as_array() else {
        if format == WireFormat::Responses {
            if let Some(text) = body[field].as_str().filter(|text| envelope_hint(text)) {
                rejected_file_carriers.push(text.to_string());
            }
        }
        return;
    };
    let mut leading_instructions = true;
    for (index, message) in messages.iter().enumerate() {
        if !is_message(message) {
            leading_instructions = false;
            continue;
        }
        let system = matches!(format, WireFormat::Responses | WireFormat::Chat)
            && matches!(message["role"].as_str(), Some("system" | "developer"));
        let user = message["role"] == "user";
        if !system && !user {
            leading_instructions = false;
            continue;
        }
        let key = if format == WireFormat::Gemini {
            "parts"
        } else {
            "content"
        };
        let content_path = format!("/{field}/{index}/{key}");
        let mut selected = content_nodes(body, &content_path, format, system);
        if system {
            nodes.extend(selected);
        } else if leading_instructions {
            // Every block must be text. Normally the entire message is instructions; only
            // the verified Desktop first-block envelope permits following context or chat.
            let text_only = match body.pointer(&content_path) {
                Some(Value::String(_)) => true,
                Some(Value::Array(blocks)) => blocks.len() == selected.len(),
                _ => false,
            };
            // Desktop's verified first block wraps global instructions even when later text
            // blocks contain git context or the initial chat. Those blocks remain opaque.
            if text_only
                && matches!(body.pointer(&content_path), Some(Value::Array(_)))
                && selected
                    .first()
                    .is_some_and(|node| node.text.starts_with(CLAUDE_GLOBAL_PREFIX))
            {
                let mut head = selected.remove(0);
                head.message = Some(format!("/{field}/{index}"));
                nodes.push(head);
                rejected_file_carriers.extend(
                    selected
                        .into_iter()
                        .filter(|node| envelope_hint(&node.text))
                        .map(|node| node.text),
                );
                leading_instructions = false;
            } else if text_only
                && !selected.is_empty()
                && selected.iter().all(|node| envelope_hint(&node.text))
            {
                for node in &mut selected {
                    node.message = Some(format!("/{field}/{index}"));
                }
                nodes.extend(selected);
            } else {
                // Any text that mentions the file header but is not a clean carrier blocks
                // request-level injection: the client may already carry the instructions.
                rejected_file_carriers.extend(
                    selected
                        .into_iter()
                        .filter(|node| node.text.contains("Contents of "))
                        .map(|node| node.text),
                );
                leading_instructions = false;
            }
        }
    }
    // Resumed or compacted sessions can carry the same global instruction envelope in a later
    // user message instead of a leading one. A message whose text blocks are all file envelopes
    // is a carrier wherever it sits; ordinary chat, tool results and mixed blocks stay opaque.
    let already: std::collections::HashSet<String> = nodes
        .iter()
        .filter_map(|node| node.message.clone())
        .collect();
    for (index, message) in messages.iter().enumerate() {
        if !is_message(message) || message["role"] != "user" {
            continue;
        }
        let id = format!("/{field}/{index}");
        if already.contains(&id) {
            continue;
        }
        let key = if format == WireFormat::Gemini {
            "parts"
        } else {
            "content"
        };
        let content_path = format!("/{field}/{index}/{key}");
        let mut selected = content_nodes(body, &content_path, format, false);
        if selected.is_empty() || !selected.iter().all(|node| envelope_hint(&node.text)) {
            continue;
        }
        let text_only = match body.pointer(&content_path) {
            Some(Value::String(_)) => true,
            Some(Value::Array(blocks)) => blocks.len() == selected.len(),
            _ => false,
        };
        if !text_only {
            continue;
        }
        for node in &mut selected {
            node.message = Some(id.clone());
        }
        nodes.extend(selected);
    }
}

pub(crate) fn select(body: &Value, format: WireFormat) -> Result<Selection, String> {
    if !body.is_object() {
        return Err("prompt rewriter: body must be an object".into());
    }
    let mut selected = Selection::default();
    match format {
        WireFormat::Anthropic => {
            selected.nodes = content_nodes(body, "/system", format, true);
            messages(
                body,
                "messages",
                format,
                &mut selected.nodes,
                &mut selected.rejected_file_carriers,
            );
        }
        WireFormat::Responses => {
            string_node(
                body,
                "/instructions",
                "/instructions",
                true,
                &mut selected.nodes,
            );
            messages(
                body,
                "input",
                format,
                &mut selected.nodes,
                &mut selected.rejected_file_carriers,
            );
        }
        WireFormat::Chat => messages(
            body,
            "messages",
            format,
            &mut selected.nodes,
            &mut selected.rejected_file_carriers,
        ),
        WireFormat::Gemini => {
            selected.nodes = content_nodes(body, "/systemInstruction/parts", format, true);
            messages(
                body,
                "contents",
                format,
                &mut selected.nodes,
                &mut selected.rejected_file_carriers,
            );
        }
    }
    let size = selected
        .nodes
        .iter()
        .map(|node| node.text.len())
        .chain(selected.rejected_file_carriers.iter().map(String::len))
        .try_fold(0usize, |sum, size| sum.checked_add(size))
        .ok_or("prompt rewriter: text size overflow")?;
    if size > 8 * 1024 * 1024 {
        return Err("prompt rewriter: direct text exceeds 8 MiB".into());
    }
    Ok(selected)
}

fn remove_at(body: &mut Value, path: &str) -> Result<(), String> {
    let (parent, key) = path
        .rsplit_once('/')
        .ok_or("prompt rewriter: invalid text path")?;
    match body.pointer_mut(parent) {
        Some(Value::Object(object)) => {
            object.remove(key);
        }
        Some(Value::Array(array)) => {
            let index: usize = key
                .parse()
                .map_err(|_| "prompt rewriter: invalid text index")?;
            if index >= array.len() {
                return Err("prompt rewriter: text index no longer exists".into());
            }
            array.remove(index);
        }
        _ => return Err("prompt rewriter: text container no longer exists".into()),
    }
    Ok(())
}

fn empty(content: Option<&Value>) -> bool {
    match content {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.is_empty(),
        Some(Value::Array(array)) => array.is_empty(),
        _ => false,
    }
}

/// All setters run before any removals, avoiding numeric-index path shifts across carriers.
pub(crate) fn commit(
    body: &mut Value,
    format: WireFormat,
    selection: &Selection,
    texts: &[String],
    created: Option<&str>,
    system_applied: bool,
) -> Result<(), String> {
    for (node, text) in selection.nodes.iter().zip(texts) {
        if text != &node.text && !text.is_empty() {
            *body
                .pointer_mut(&node.path)
                .ok_or("prompt rewriter: text path no longer exists")? = json!(text);
            // Changed text must not retain a cache marker for its former contents.
            if node.remove_path != node.path {
                if let Some(block) = body
                    .pointer_mut(&node.remove_path)
                    .and_then(Value::as_object_mut)
                {
                    block.remove("cache_control");
                }
            }
        }
    }
    // Discovery order is native array order within each distinct container.
    for (node, text) in selection.nodes.iter().zip(texts).rev() {
        if text.is_empty() && (text != &node.text || (system_applied && node.system)) {
            remove_at(body, &node.remove_path)?;
        }
    }
    if let Some(text) = created.filter(|text| !text.is_empty()) {
        create_system(body, format, text)?;
    }
    if system_applied {
        cleanup_system(body, format);
    }
    Ok(())
}

/// An empty, readable source has no client segment to replace. Add a separate user-level
/// message, never splice into ordinary chat or claim the client supplied a file envelope.
/// This runs after setters/removals so inserting at index zero cannot invalidate their paths.
pub(crate) fn prepend_user_instructions(
    body: &mut Value,
    format: WireFormat,
    text: &str,
) -> Result<(), String> {
    let (field, message) = match format {
        WireFormat::Anthropic | WireFormat::Chat => (
            "messages",
            json!({"role":"user","content":[{"type":"text","text":text}]}),
        ),
        WireFormat::Responses => (
            "input",
            json!({"role":"user","content":[{"type":"input_text","text":text}]}),
        ),
        WireFormat::Gemini => ("contents", json!({"role":"user","parts":[{"text":text}]})),
    };
    if format == WireFormat::Responses {
        if let Some(original) = body[field].as_str() {
            body[field] = json!([{"role":"user","content":original}]);
        }
    }
    if body.get(field).is_none_or(Value::is_null) {
        body[field] = json!([]);
    }
    let messages = body[field]
        .as_array_mut()
        .ok_or("prompt rewriter: user instruction container must be an array")?;
    // Idempotent: a conversation that already carries this exact block never gets a second one.
    if messages.iter().any(|entry| message_mentions(entry, text)) {
        return Ok(());
    }
    // A leading system/developer turn must stay first; operator instructions go after it.
    let insert_at = messages
        .iter()
        .position(|entry| !matches!(entry["role"].as_str(), Some("system" | "developer")))
        .unwrap_or(messages.len());
    messages.insert(insert_at, message);
    Ok(())
}

fn message_mentions(message: &Value, needle: &str) -> bool {
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        return text.contains(needle);
    }
    // Anthropic/Chat/Responses keep text in `content`, Gemini in `parts`.
    ["content", "parts"].iter().any(|key| {
        message
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains(needle))
                })
            })
    })
}

fn create_system(body: &mut Value, format: WireFormat, text: &str) -> Result<(), String> {
    match format {
        WireFormat::Anthropic => match body.get_mut("system") {
            None | Some(Value::Null) => body["system"] = json!(text),
            Some(Value::Array(blocks)) => blocks.push(json!({"type":"text","text":text})),
            _ => return Err("prompt rewriter: unsupported system container".into()),
        },
        WireFormat::Responses => {
            if body
                .get("instructions")
                .is_some_and(|value| !value.is_null() && !value.is_string())
            {
                return Err("prompt rewriter: unsupported instructions container".into());
            }
            body["instructions"] = json!(text);
        }
        WireFormat::Chat => {
            if body.get("messages").is_none() {
                body["messages"] = json!([]);
            }
            body["messages"]
                .as_array_mut()
                .ok_or("prompt rewriter: messages must be an array")?
                .insert(0, json!({"role":"system","content":text}));
        }
        WireFormat::Gemini => {
            if body.get("systemInstruction").is_none_or(Value::is_null) {
                body["systemInstruction"] = json!({"parts":[]});
            }
            body["systemInstruction"]
                .as_object_mut()
                .ok_or("prompt rewriter: systemInstruction must be an object")?
                .entry("parts")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or("prompt rewriter: systemInstruction.parts must be an array")?
                .push(json!({"text":text}));
        }
    }
    Ok(())
}

fn cleanup_system(body: &mut Value, format: WireFormat) {
    match format {
        WireFormat::Anthropic => {
            if empty(body.get("system")) {
                body.as_object_mut().unwrap().remove("system");
            }
        }
        WireFormat::Responses | WireFormat::Chat => {
            if format == WireFormat::Responses && empty(body.get("instructions")) {
                body.as_object_mut().unwrap().remove("instructions");
            }
            let field = if format == WireFormat::Responses {
                "input"
            } else {
                "messages"
            };
            if let Some(array) = body.get_mut(field).and_then(Value::as_array_mut) {
                array.retain(|message| {
                    !(is_message(message)
                        && matches!(message["role"].as_str(), Some("system" | "developer"))
                        && empty(message.get("content")))
                });
            }
        }
        WireFormat::Gemini => {
            if body
                .get("systemInstruction")
                .is_some_and(|system| empty(system.get("parts")))
            {
                body.as_object_mut().unwrap().remove("systemInstruction");
            }
        }
    }
}
