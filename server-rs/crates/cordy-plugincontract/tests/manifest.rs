//! Port of `manifest_test.go` and `examples_test.go`.

use std::collections::HashSet;
use std::path::PathBuf;

use cordy_plugincontract::{
    host_capabilities, is_known_event, net_domains, parse_manifest, validate_scope, Capabilities,
    CONFIG_ENUM, MAX_MANIFEST_SIZE, RESOURCE_SKILL, SCOPE_AGENTS_READ, SCOPE_COMMENTS_READ,
    SCOPE_COMMENTS_WRITE, SCOPE_ISSUES_READ, SCOPE_ISSUES_WRITE, SCOPE_MEMBERS_READ,
    SCOPE_STORAGE_USER, SCOPE_STORAGE_WORKSPACE, SCOPE_TASKS_READ, SCOPE_TASKS_WRITE,
    SURFACE_ISSUE_PANEL, SURFACE_MODAL, SURFACE_SIDEBAR_PANEL, TRANSPORT_HTTP, TRANSPORT_MCP,
    TRIGGER_AGENT, TRIGGER_EVENT, TRIGGER_MANUAL, TRIGGER_UI,
};

// validManifest is the reference document every negative case mutates. Keeping
// one source avoids a test that passes because it drifted from the real shape.
const VALID_MANIFEST: &str = r#"{
  "manifest_version": 1,
  "key": "com.example.hello",
  "name": "Hello Panel",
  "description": "A greeting panel.",
  "version": "1.0.0",
  "author": { "name": "example", "url": "https://example.com" },
  "icon": "icon.svg",
  "scopes": ["issues:read", "comments:write", "storage:user", "net:example.com"],
  "config": {
    "repo": { "type": "string", "label": "GitHub Repo", "required": true },
    "token": { "type": "secret", "label": "Access Token", "required": true },
    "mode": { "type": "enum", "label": "Mode", "options": ["fast", "thorough"] }
  },
  "contributes": {
    "surfaces": [{
      "key": "hello", "type": "issue_panel", "name": "Hello",
      "entry": "ui/main.js", "platforms": ["web", "desktop"]
    }],
    "hooks": [{
      "key": "summarize_thread",
      "name": "Summarize",
      "description": "Compress the issue discussion into bullet points.",
      "input_schema": { "type": "object", "properties": { "issue_id": { "type": "string" } } },
      "triggers": ["ui", "manual", "agent"],
      "transport": { "type": "http", "url": "https://example.com/hooks/summarize" },
      "timeout_ms": 10000
    }],
    "resources": [
      { "type": "skill", "key": "pr-review", "entry": "skills/pr-review/SKILL.md" }
    ]
  }
}"#;

fn mutate(edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut doc: serde_json::Value =
        serde_json::from_str(VALID_MANIFEST).expect("decode reference manifest");
    edit(&mut doc);
    serde_json::to_vec(&doc).expect("encode mutated manifest")
}

#[test]
fn parse_manifest_accepts_reference_document() {
    let (manifest, canonical) = parse_manifest(VALID_MANIFEST.as_bytes()).expect("ParseManifest");
    assert_eq!(manifest.key, "com.example.hello");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.contributes.surfaces.len(), 1);
    assert_eq!(manifest.contributes.hooks.len(), 1);
    assert_eq!(manifest.contributes.resources.len(), 1);
    assert!(!canonical.is_empty(), "canonical manifest is empty");
    // The canonical form must reparse: it is what an installation stores and
    // later reads back as the consented snapshot.
    parse_manifest(&canonical).expect("canonical manifest does not reparse");
}

#[test]
fn config_schema_preserves_declaration_order() {
    let (manifest, canonical) = parse_manifest(VALID_MANIFEST.as_bytes()).expect("ParseManifest");
    let want = ["repo", "token", "mode"];
    assert_eq!(manifest.config.fields.len(), want.len());
    for (field, key) in manifest.config.fields.iter().zip(want) {
        assert_eq!(field.key, key);
    }
    // The generated form order must survive the snapshot round trip, otherwise
    // the same plugin renders its fields differently after an upgrade.
    let canonical_str = String::from_utf8(canonical).expect("utf8 canonical");
    let repo = canonical_str.find("\"repo\"").expect("repo in canonical");
    let token = canonical_str.find("\"token\"").expect("token in canonical");
    assert!(
        repo < token,
        "canonical config lost declaration order: {canonical_str}"
    );
    let field = manifest.config.field("mode").expect("enum field preserved");
    assert_eq!(field.field_type, CONFIG_ENUM);
    assert_eq!(field.options.len(), 2);
}

#[test]
fn parse_manifest_rejects_malformed_documents() {
    struct Case {
        name: &'static str,
        raw: Vec<u8>,
        want: &'static str,
    }

    fn surfaces(doc: &mut serde_json::Value) -> &mut Vec<serde_json::Value> {
        doc["contributes"]["surfaces"].as_array_mut().unwrap()
    }
    fn hooks(doc: &mut serde_json::Value) -> &mut Vec<serde_json::Value> {
        doc["contributes"]["hooks"].as_array_mut().unwrap()
    }

    let cases = vec![
        Case {
            name: "empty",
            raw: Vec::new(),
            want: "empty",
        },
        Case {
            name: "unknown top-level field",
            raw: br#"{"manifest_version":1,"surprise":true}"#.to_vec(),
            want: "unknown field",
        },
        Case {
            name: "trailing JSON",
            raw: [VALID_MANIFEST.as_bytes(), br#"{"extra":1}"#.as_ref()].concat(),
            want: "trailing",
        },
        Case {
            name: "unknown config field property",
            raw: mutate(|doc| {
                doc["config"] = serde_json::json!({
                    "a": {"type": "string", "label": "A", "secret": true}
                });
            }),
            want: "unknown field",
        },
        Case {
            name: "wrong manifest version",
            raw: mutate(|doc| doc["manifest_version"] = serde_json::json!(2)),
            want: "manifest_version",
        },
        Case {
            name: "non reverse-DNS key",
            raw: mutate(|doc| doc["key"] = serde_json::json!("hello")),
            want: "reverse-DNS",
        },
        Case {
            name: "non semver version",
            raw: mutate(|doc| doc["version"] = serde_json::json!("1.0")),
            want: "semantic versioning",
        },
        Case {
            name: "overlong name",
            raw: mutate(|doc| doc["name"] = serde_json::json!("n".repeat(161))),
            want: "exceeds",
        },
        Case {
            name: "unknown scope",
            raw: mutate(|doc| {
                doc["scopes"] = serde_json::json!(["issues:read", "billing:write"]);
            }),
            want: "unsupported scope",
        },
        Case {
            name: "malformed net scope",
            raw: mutate(|doc| {
                doc["scopes"] = serde_json::json!(["net:https://example.com"]);
            }),
            want: "invalid domain",
        },
        Case {
            name: "duplicate scope",
            raw: mutate(|doc| {
                doc["scopes"] = serde_json::json!(["issues:read", "issues:read"]);
            }),
            want: "duplicate",
        },
        Case {
            name: "empty scopes",
            raw: mutate(|doc| doc["scopes"] = serde_json::json!([])),
            want: "must not be empty",
        },
        Case {
            name: "unsupported config type",
            raw: mutate(|doc| {
                doc["config"] = serde_json::json!({"a": {"type": "json", "label": "A"}});
            }),
            want: "unsupported",
        },
        Case {
            name: "enum without options",
            raw: mutate(|doc| {
                doc["config"] = serde_json::json!({"a": {"type": "enum", "label": "A"}});
            }),
            want: "options must not be empty",
        },
        Case {
            name: "no contributions",
            raw: mutate(|doc| doc["contributes"] = serde_json::json!({})),
            want: "at least one",
        },
        Case {
            name: "unsupported surface type",
            raw: mutate(|doc| surfaces(doc)[0]["type"] = serde_json::json!("fullscreen")),
            want: "unsupported",
        },
        Case {
            name: "surface entry escapes the package",
            raw: mutate(|doc| surfaces(doc)[0]["entry"] = serde_json::json!("../../etc/passwd")),
            want: "path traversal",
        },
        Case {
            name: "version over the column bound",
            raw: mutate(|doc| {
                // A legal semver whose build metadata pushes it past the
                // plugin_installation.version cap.
                doc["version"] = serde_json::json!(format!("1.0.0+{}", "b".repeat(64)));
            }),
            want: "version exceeds",
        },
        Case {
            name: "surface entry is an HTML document",
            raw: mutate(|doc| surfaces(doc)[0]["entry"] = serde_json::json!("ui/index.html")),
            want: "must be a .js or .mjs script",
        },
        Case {
            name: "hook transport on a subdomain of a net: scope",
            raw: mutate(|doc| {
                hooks(doc)[0]["transport"] = serde_json::json!({
                    "type": "http", "url": "https://api.example.com/hooks/summarize"
                });
            }),
            want: "not covered by a net: scope",
        },
        Case {
            name: "surface entry is an absolute URL",
            raw: mutate(|doc| {
                surfaces(doc)[0]["entry"] = serde_json::json!("https://evil.test/index.html");
            }),
            want: "relative path",
        },
        Case {
            name: "unsupported trigger",
            raw: mutate(|doc| hooks(doc)[0]["triggers"] = serde_json::json!(["cron"])),
            want: "unsupported trigger",
        },
        Case {
            name: "event trigger without events",
            raw: mutate(|doc| hooks(doc)[0]["triggers"] = serde_json::json!(["event"])),
            want: "events must not be empty",
        },
        Case {
            name: "events without the event trigger",
            raw: mutate(|doc| hooks(doc)[0]["events"] = serde_json::json!(["issue.created"])),
            want: "requires the event trigger",
        },
        Case {
            name: "unknown event",
            raw: mutate(|doc| {
                hooks(doc)[0]["triggers"] = serde_json::json!(["event"]);
                hooks(doc)[0]["events"] = serde_json::json!(["issue.exploded"]);
            }),
            want: "unsupported event",
        },
        Case {
            name: "hook transport outside the granted net scope",
            raw: mutate(|doc| {
                hooks(doc)[0]["transport"] =
                    serde_json::json!({"type": "http", "url": "https://evil.test/hook"});
            }),
            want: "not covered by a net: scope",
        },
        Case {
            name: "plaintext hook transport",
            raw: mutate(|doc| {
                hooks(doc)[0]["transport"] =
                    serde_json::json!({"type": "http", "url": "http://example.com/hook"});
            }),
            want: "HTTPS",
        },
        Case {
            name: "duplicate hook key",
            raw: mutate(|doc| {
                let clone = hooks(doc)[0].clone();
                hooks(doc).push(clone);
            }),
            want: "duplicate hook key",
        },
        Case {
            name: "unsupported resource type",
            raw: mutate(|doc| {
                doc["contributes"]["resources"] = serde_json::json!([
                    {"type": "font", "key": "a", "entry": "a"}
                ]);
            }),
            want: "unsupported",
        },
        Case {
            name: "resource entry does not match its key",
            raw: mutate(|doc| {
                doc["contributes"]["resources"] = serde_json::json!([
                    {"type": "skill", "key": "review", "entry": "skills/other/SKILL.md"}
                ]);
            }),
            want: "must be",
        },
    ];

    for case in cases {
        let err =
            parse_manifest(&case.raw).expect_err(&format!("ParseManifest accepted {}", case.name));
        assert!(
            err.to_string().contains(case.want),
            "{}: error = {err}, want it to mention {:?}",
            case.name,
            case.want
        );
    }
}

#[test]
fn parse_manifest_rejects_oversized_document() {
    let oversized = vec![b' '; MAX_MANIFEST_SIZE + 1];
    let err = parse_manifest(&oversized).expect_err("oversized manifest must be rejected");
    assert!(
        err.to_string().contains("exceeds"),
        "oversized manifest error = {err}"
    );
}

#[test]
fn validate_scope_accepts_defined_scopes_only() {
    let valid = [
        SCOPE_ISSUES_READ,
        SCOPE_ISSUES_WRITE,
        SCOPE_COMMENTS_READ,
        SCOPE_COMMENTS_WRITE,
        SCOPE_TASKS_READ,
        SCOPE_TASKS_WRITE,
        SCOPE_AGENTS_READ,
        SCOPE_MEMBERS_READ,
        SCOPE_STORAGE_USER,
        SCOPE_STORAGE_WORKSPACE,
        "net:example.com",
        "net:api.example.co.uk",
    ];
    for scope in valid {
        validate_scope(scope).unwrap_or_else(|e| panic!("ValidateScope({scope:?}) = {e}"));
    }
    let invalid = [
        "",
        "issues",
        "issues:delete",
        "storage:global",
        "net:",
        "net:localhost",
        "net:EXAMPLE.com",
        "net:example.com/path",
        "net:*.example.com",
        "NET:example.com",
    ];
    for scope in invalid {
        assert!(
            validate_scope(scope).is_err(),
            "ValidateScope({scope:?}) accepted an invalid scope"
        );
    }
}

#[test]
fn net_domains_only_returns_net_scopes() {
    let domains = net_domains(&[
        SCOPE_ISSUES_READ.to_string(),
        "net:example.com".to_string(),
        SCOPE_STORAGE_USER.to_string(),
        "net:api.example.com".to_string(),
    ]);
    assert_eq!(domains, vec!["example.com", "api.example.com"]);
}

// The gate's job, stated without naming today's configuration:
// everything the host cannot run is reported, everything it can run is not,
// and all of it arrives at once.
#[test]
fn check_capabilities_reports_every_unavailable_contribution() {
    let (manifest, _) = parse_manifest(VALID_MANIFEST.as_bytes()).expect("ParseManifest");

    // Against a host that supports nothing, every declared contribution in the
    // fixture must be named — not the first one found.
    let unavailable = manifest
        .check_capabilities(&Capabilities::default())
        .expect_err("a host with no capabilities accepted contributions it cannot run");

    let mut want_all = Vec::new();
    for surface in &manifest.contributes.surfaces {
        want_all.push(format!("surface {}", surface.surface_type));
    }
    for hook in &manifest.contributes.hooks {
        for trigger in &hook.triggers {
            want_all.push(format!("hook trigger {trigger}"));
        }
        want_all.push(format!("hook transport {}", hook.transport.transport_type));
    }
    for resource in &manifest.contributes.resources {
        want_all.push(format!("resource {}", resource.resource_type));
    }
    for want in &want_all {
        assert!(
            unavailable.missing.iter().any(|m| m == want),
            "missing = {:?}, want it to include {want:?} — every gap must be reported at once, not one install at a time",
            unavailable.missing
        );
    }

    // Against the real host set: whatever is shipped must NOT be reported, and
    // whatever is not shipped must be. Derived from host_capabilities rather
    // than restated, so a flip changes one place and this keeps testing the gate.
    let host = host_capabilities();
    let reported = match manifest.check_capabilities(&host) {
        Ok(()) => Vec::new(),
        Err(unavailable) => unavailable.missing,
    };
    for surface in &manifest.contributes.surfaces {
        assert_gate_agrees(
            &reported,
            &format!("surface {}", surface.surface_type),
            host.surface_types.contains(&surface.surface_type),
        );
    }
    for hook in &manifest.contributes.hooks {
        for trigger in &hook.triggers {
            assert_gate_agrees(
                &reported,
                &format!("hook trigger {trigger}"),
                host.hook_triggers.contains(trigger),
            );
        }
        assert_gate_agrees(
            &reported,
            &format!("hook transport {}", hook.transport.transport_type),
            host.hook_transport.contains(&hook.transport.transport_type),
        );
    }
    for resource in &manifest.contributes.resources {
        assert_gate_agrees(
            &reported,
            &format!("resource {}", resource.resource_type),
            host.resource_types.contains(&resource.resource_type),
        );
    }

    let full = Capabilities {
        surface_types: HashSet::from([
            SURFACE_ISSUE_PANEL.to_string(),
            SURFACE_SIDEBAR_PANEL.to_string(),
            SURFACE_MODAL.to_string(),
        ]),
        hook_triggers: HashSet::from([
            TRIGGER_UI.to_string(),
            TRIGGER_MANUAL.to_string(),
            TRIGGER_AGENT.to_string(),
            TRIGGER_EVENT.to_string(),
        ]),
        hook_transport: HashSet::from([TRANSPORT_HTTP.to_string(), TRANSPORT_MCP.to_string()]),
        resource_types: HashSet::from([RESOURCE_SKILL.to_string()]),
    };
    manifest
        .check_capabilities(&full)
        .expect("CheckCapabilities with full host support");
}

// Pins the gate to the host set in both directions: a shipped capability
// reported as missing would fail every install of a plugin the host can
// actually run, and an unshipped one left unreported would install a
// contribution that silently never fires.
fn assert_gate_agrees(reported: &[String], name: &str, shipped: bool) {
    if shipped {
        assert!(
            !reported.iter().any(|r| r == name),
            "{name:?} is shipped by this host but was reported unavailable"
        );
    } else {
        assert!(
            reported.iter().any(|r| r == name),
            "{name:?} is NOT shipped by this host but was not reported — it would install and never fire"
        );
    }
}

// An event subscription delivers the same content the Action API would have
// required a scope to read: issue.* carries the description, comment.created
// carries the body. Without this check, subscribing was a way to receive what
// reading was never granted.
#[test]
fn event_subscription_requires_the_matching_read_scope() {
    fn manifest(scopes: &str, events: &str) -> Vec<u8> {
        format!(
            r#"{{
                "manifest_version": 1,
                "key": "com.example.events",
                "name": "Events",
                "description": "d",
                "version": "1.0.0",
                "author": {{"name": "example"}},
                "scopes": {scopes},
                "contributes": {{"hooks": [{{
                    "key": "watch",
                    "name": "Watch",
                    "description": "Watch things happen.",
                    "triggers": ["event"],
                    "events": {events},
                    "transport": {{"type": "http", "url": "https://example.com/hooks/watch"}}
                }}]}}
            }}"#
        )
        .into_bytes()
    }

    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "issue event without issues:read",
            r#"["net:example.com"]"#,
            r#"["issue.created"]"#,
            true,
        ),
        (
            "issue event with issues:read",
            r#"["issues:read", "net:example.com"]"#,
            r#"["issue.created"]"#,
            false,
        ),
        (
            "comment event without comments:read",
            r#"["issues:read", "net:example.com"]"#,
            r#"["comment.created"]"#,
            true,
        ),
        (
            "comment event with comments:read",
            r#"["comments:read", "net:example.com"]"#,
            r#"["comment.created"]"#,
            false,
        ),
        (
            "task event without tasks:read",
            r#"["net:example.com"]"#,
            r#"["task.failed"]"#,
            true,
        ),
        (
            "task event with tasks:read",
            r#"["tasks:read", "net:example.com"]"#,
            r#"["task.failed"]"#,
            false,
        ),
        (
            "one of several events unscoped",
            r#"["issues:read", "net:example.com"]"#,
            r#"["issue.created", "comment.created"]"#,
            true,
        ),
    ];

    for (name, scopes, events, want_err) in cases {
        let result = parse_manifest(&manifest(scopes, events));
        if *want_err {
            assert!(
                result.is_err(),
                "{name}: subscribing to content the manifest may not read must be refused at install"
            );
        } else {
            result.unwrap_or_else(|e| {
                panic!("{name}: a properly scoped subscription must parse: {e}")
            });
        }
    }
}

#[test]
fn known_events_match_the_dispatcher_contract() {
    for event in [
        "issue.created",
        "issue.updated",
        "issue.status_changed",
        "comment.created",
        "task.started",
        "task.completed",
        "task.failed",
    ] {
        assert!(is_known_event(event), "{event:?} must be known");
    }
    assert!(!is_known_event("issue.exploded"));
    assert!(!is_known_event(""));
}

// The shipped examples are documentation, and documentation that does not
// parse is worse than none. This also catches the failure mode a capability
// flip introduces: an example declaring something the host has not shipped yet
// would install nowhere, and nothing else in the build would say so.
#[test]
fn shipped_examples_parse_and_install_on_this_host() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/plugins");
    let entries =
        std::fs::read_dir(&root).unwrap_or_else(|e| panic!("read examples directory: {e}"));

    let mut found = 0;
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let path = entry.path().join("cordy.plugin.json");
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        found += 1;

        let (manifest, _) = parse_manifest(&raw).unwrap_or_else(|e| {
            panic!(
                "{}: example manifest does not parse: {e}",
                entry.path().display()
            )
        });
        manifest.check_capabilities(&host_capabilities()).unwrap_or_else(|e| {
            panic!(
                "{}: example declares something this host cannot run, so it could not be installed: {e}",
                entry.path().display()
            )
        });
        // A surface entry that is not in the example's own directory is a
        // broken example: the reader copies it and gets a blank panel.
        for surface in &manifest.contributes.surfaces {
            let entry_path = entry.path().join(&surface.entry);
            assert!(
                entry_path.exists(),
                "{}: surface {:?} points at {}, which is not in the example",
                entry.path().display(),
                surface.key,
                surface.entry
            );
        }
    }

    assert!(
        found > 0,
        "no example manifests were found; this test would pass vacuously"
    );
}
