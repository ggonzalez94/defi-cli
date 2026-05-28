//! Tests for `defi-schema` — the machine-readable command schema data model and its
//! clap-free helper functions (Go source: `internal/schema`).
//!
//! # Success criteria (the contract this module must satisfy)
//!
//! 1. **Serde model field names & declaration order.** Every schema struct
//!    (`CommandSchema`, `FlagSchema`, `CommandMetadata`, `AuthRequirement`,
//!    `InputConstraint`, `FlagMetadata`, `TypeSchema`, `SchemaField`) serializes to JSON
//!    with the EXACT snake_case key names from Go's struct tags, in struct DECLARATION
//!    order. Notably `use`->`use`, `type`->`type`, `enum_values`->`enum`. JSON uses
//!    2-space indent (the global render contract).
//!
//! 2. **`omitempty` semantics.** Optional/empty collections, `false` bools, empty strings,
//!    and `None` options are OMITTED from JSON (matching Go `,omitempty`). Required keys
//!    (`path`, `use`, `short`, `name`, `type`, `usage`, `kind`, `schema`) are ALWAYS
//!    present. The one deliberate exception, mirroring Go: a `SchemaField.default` of
//!    empty-string IS emitted (Go `omitempty` on `any` only omits `nil`, not `""`), so
//!    `default` is modeled as an `Option<Value>` and emitted when `Some("")`.
//!
//! 3. **Ordered `when` maps.** `AuthRequirement.when` / `InputConstraint.when` preserve
//!    insertion order (IndexMap), so conditional metadata is deterministic.
//!
//! 4. **Round-trip stability.** Deserialize -> serialize of a representative schema node
//!    reproduces the original JSON byte-for-byte (anchored to the real golden fixture
//!    shapes: `approvals plan`-style request fields and the wallet/signer auth block).
//!
//! 5. **Enum inference from usage** (`infer_enum_values`, port of Go `inferEnumValues`):
//!    - `"Provider (aave|morpho)"` -> `["aave","morpho"]` (pipe form).
//!    - `"type (exact-input | exact-output)"` -> trims whitespace per token.
//!    - `"limit (max=100, min=1)"` -> `["max","min"]` (`k=v,` form -> keys).
//!    - no parens / empty body / no separators -> `None`.
//!
//! 6. **Enum token sanitization** (`sanitize_enum_value`, port of `sanitizeEnumValue`):
//!    first whitespace-delimited word, with trailing `,;.)]` stripped; empty -> `""`.
//!
//! 7. **Enum tag splitting** (`split_schema_enum`, port of `splitSchemaEnum`):
//!    comma-separated, trimmed, empties dropped.
//!
//! 8. **stringSlice default parsing** (`parse_string_slice_default`, port of
//!    `parseStringSliceDefault`): `"[]"`/`""` -> `[]`; `"[a, b,c]"` -> `["a","b","c"]`;
//!    surrounding brackets stripped; empties dropped.
//!
//! 9. **Full golden-fixture round-trip (the primary contract oracle).** The complete
//!    `data` node of the real `rust/tests/golden/schema.json` capture (the entire Go
//!    command tree — every flag, request/response `TypeSchema`, auth block, input
//!    constraint, `items`, `additional_properties`, and `when` map) deserializes into
//!    `CommandSchema` and re-serializes to a byte-identical, order-preserving JSON value.
//!    This is far stronger than the hand-built shapes in (4): it proves the serde model is
//!    contract-complete (no dropped/renamed/reordered field, omitempty parity) against the
//!    real Go output. Requires `serde_json/preserve_order` (enabled workspace-wide) so the
//!    comparison is order-sensitive rather than masked by a sorted `BTreeMap`.

use super::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// 1 + 2: serde field names, declaration order, omitempty
// ---------------------------------------------------------------------------

#[test]
fn command_schema_minimal_omits_empty_and_keeps_required_keys() {
    let cmd = CommandSchema {
        path: "defi yield".into(),
        r#use: "yield".into(),
        short: "yield cmds".into(),
        ..Default::default()
    };
    let v = serde_json::to_value(&cmd).unwrap();
    let obj = v.as_object().unwrap();

    // Required keys always present.
    assert_eq!(obj.get("path").unwrap(), "defi yield");
    assert_eq!(obj.get("use").unwrap(), "yield");
    assert_eq!(obj.get("short").unwrap(), "yield cmds");

    // omitempty: none of these should appear when empty/false/None.
    for absent in [
        "aliases",
        "mutation",
        "input_modes",
        "input_constraints",
        "auth",
        "request",
        "response",
        "flags",
        "subcommands",
    ] {
        assert!(
            !obj.contains_key(absent),
            "expected `{absent}` to be omitted when empty"
        );
    }
}

#[test]
fn command_schema_serializes_keys_in_declaration_order() {
    let cmd = CommandSchema {
        path: "defi yield plan".into(),
        r#use: "plan".into(),
        short: "create a yield action plan".into(),
        aliases: vec!["p".into()],
        mutation: true,
        input_modes: vec!["flags".into(), "json".into()],
        input_constraints: vec![InputConstraint {
            kind: "exactly_one_of".into(),
            fields: vec!["wallet".into(), "from_address".into()],
            ..Default::default()
        }],
        auth: vec![AuthRequirement {
            kind: "wallet".into(),
            ..Default::default()
        }],
        request: Some(TypeSchema {
            r#type: "object".into(),
            ..Default::default()
        }),
        response: Some(TypeSchema {
            r#type: "object".into(),
            ..Default::default()
        }),
        flags: vec![FlagSchema {
            name: "provider".into(),
            r#type: "string".into(),
            usage: "Yield provider".into(),
            ..Default::default()
        }],
        subcommands: vec![],
    };

    let pretty = serde_json::to_string_pretty(&cmd).unwrap();
    // Match `"key":` (with colon) so a key name appearing as a VALUE (e.g. the
    // string "flags" inside input_modes) can't produce a false position.
    let order: Vec<&str> = [
        "\"path\":",
        "\"use\":",
        "\"short\":",
        "\"aliases\":",
        "\"mutation\":",
        "\"input_modes\":",
        "\"input_constraints\":",
        "\"auth\":",
        "\"request\":",
        "\"response\":",
        "\"flags\":",
    ]
    .into_iter()
    .map(|k| {
        let idx = pretty.find(k).unwrap_or_else(|| panic!("missing key {k}"));
        (k, idx)
    })
    .scan(0usize, |prev, (k, idx)| {
        assert!(idx >= *prev, "key {k} out of declaration order");
        *prev = idx;
        Some(k)
    })
    .collect();
    assert_eq!(order.len(), 11);
}

#[test]
fn flag_schema_renames_type_and_enum_and_omits_empty() {
    let flag = FlagSchema {
        name: "json".into(),
        r#type: "bool".into(),
        usage: "Output JSON (default)".into(),
        default: Some(json!(false)),
        scope: "inherited".into(),
        ..Default::default()
    };
    let v = serde_json::to_value(&flag).unwrap();
    let obj = v.as_object().unwrap();

    // `r#type` must serialize as `type`.
    assert_eq!(obj.get("type").unwrap(), "bool");
    assert!(!obj.contains_key("r#type"));
    assert_eq!(obj.get("name").unwrap(), "json");
    assert_eq!(obj.get("usage").unwrap(), "Output JSON (default)");
    assert_eq!(obj.get("default").unwrap(), &json!(false));
    assert_eq!(obj.get("scope").unwrap(), "inherited");

    // omitempty
    assert!(!obj.contains_key("shorthand"));
    assert!(!obj.contains_key("required"));
    assert!(!obj.contains_key("enum"));
    assert!(!obj.contains_key("format"));
}

#[test]
fn flag_schema_enum_field_serializes_as_enum_key() {
    let flag = FlagSchema {
        name: "provider".into(),
        r#type: "string".into(),
        usage: "Yield provider".into(),
        required: true,
        enum_values: vec!["aave".into(), "morpho".into()],
        format: "provider".into(),
        ..Default::default()
    };
    let v = serde_json::to_value(&flag).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.get("enum").unwrap(), &json!(["aave", "morpho"]));
    assert!(!obj.contains_key("enum_values"));
    assert_eq!(obj.get("required").unwrap(), &json!(true));
    assert_eq!(obj.get("format").unwrap(), "provider");
}

#[test]
fn type_schema_renames_type_and_additional_properties_key() {
    let ts = TypeSchema {
        r#type: "object".into(),
        additional_properties: Some(Box::new(TypeSchema {
            r#type: "string".into(),
            ..Default::default()
        })),
        ..Default::default()
    };
    let v = serde_json::to_value(&ts).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.get("type").unwrap(), "object");
    // Go json tag is `additional_properties`.
    assert!(obj.contains_key("additional_properties"));
    assert!(!obj.contains_key("additionalProperties"));
    // omitempty: format/description/enum/fields/items absent.
    for absent in ["format", "description", "enum", "fields", "items"] {
        assert!(!obj.contains_key(absent), "{absent} should be omitted");
    }
}

#[test]
fn schema_field_required_schema_key_always_present() {
    let f = SchemaField {
        name: "chain".into(),
        required: true,
        default: Some(json!("")),
        description: "Chain identifier".into(),
        schema: TypeSchema {
            r#type: "string".into(),
            format: "chain".into(),
            ..Default::default()
        },
    };
    let v = serde_json::to_value(&f).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.get("name").unwrap(), "chain");
    assert_eq!(obj.get("required").unwrap(), &json!(true));
    assert_eq!(obj.get("description").unwrap(), "Chain identifier");
    // `schema` (required, no omitempty in Go) always present.
    assert!(obj.contains_key("schema"));
    assert_eq!(obj["schema"]["type"], "string");
    assert_eq!(obj["schema"]["format"], "chain");
}

#[test]
fn schema_field_emits_empty_string_default_like_go() {
    // Go `Default any json:"default,omitempty"`: empty string IS emitted because
    // omitempty on interface{} only drops nil. We model default as Option<Value>;
    // Some("") must serialize as `"default": ""`, while None is omitted.
    let with_default = SchemaField {
        name: "amount".into(),
        default: Some(json!("")),
        schema: TypeSchema {
            r#type: "string".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let v = serde_json::to_value(&with_default).unwrap();
    assert_eq!(
        v.as_object().unwrap().get("default").unwrap(),
        &json!(""),
        "empty-string default must be emitted (Go parity)"
    );

    let without_default = SchemaField {
        name: "amount".into(),
        default: None,
        schema: TypeSchema {
            r#type: "string".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let v2 = serde_json::to_value(&without_default).unwrap();
    assert!(
        !v2.as_object().unwrap().contains_key("default"),
        "None default must be omitted"
    );
}

#[test]
fn auth_requirement_keys_and_omitempty() {
    let auth = AuthRequirement {
        kind: "signer".into(),
        env_vars: vec!["DEFI_PRIVATE_KEY".into()],
        optional: true,
        description: "Local signer auth".into(),
        ..Default::default()
    };
    let v = serde_json::to_value(&auth).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.get("kind").unwrap(), "signer");
    assert_eq!(obj.get("env_vars").unwrap(), &json!(["DEFI_PRIVATE_KEY"]));
    assert_eq!(obj.get("optional").unwrap(), &json!(true));
    assert_eq!(obj.get("description").unwrap(), "Local signer auth");
    assert!(!obj.contains_key("when"), "empty when must be omitted");
}

#[test]
fn input_constraint_keys_and_omitempty() {
    let c = InputConstraint {
        kind: "exactly_one_of".into(),
        fields: vec!["wallet".into(), "from_address".into()],
        ..Default::default()
    };
    let v = serde_json::to_value(&c).unwrap();
    let obj = v.as_object().unwrap();
    assert_eq!(obj.get("kind").unwrap(), "exactly_one_of");
    assert_eq!(
        obj.get("fields").unwrap(),
        &json!(["wallet", "from_address"])
    );
    assert!(!obj.contains_key("when"));
    assert!(!obj.contains_key("description"));
}

// ---------------------------------------------------------------------------
// 3: ordered `when` maps
// ---------------------------------------------------------------------------

#[test]
fn input_constraint_when_preserves_insertion_order() {
    let mut when = IndexMap::new();
    when.insert("provider".to_string(), vec!["tempo".to_string()]);
    when.insert("zzz_first_inserted_last".to_string(), vec!["x".to_string()]);
    when.insert("aaa_inserted_after".to_string(), vec!["y".to_string()]);
    let c = InputConstraint {
        kind: "required".into(),
        fields: vec!["from_address".into()],
        when,
        description: "Tempo planning".into(),
    };
    let pretty = serde_json::to_string_pretty(&c).unwrap();
    let i_provider = pretty.find("\"provider\"").unwrap();
    let i_zzz = pretty.find("\"zzz_first_inserted_last\"").unwrap();
    let i_aaa = pretty.find("\"aaa_inserted_after\"").unwrap();
    assert!(
        i_provider < i_zzz && i_zzz < i_aaa,
        "when map must preserve insertion order, not sort keys"
    );
}

// ---------------------------------------------------------------------------
// 4: round-trip parity against real golden fixture shapes
// ---------------------------------------------------------------------------

#[test]
fn approvals_plan_request_field_round_trips() {
    // Shape taken verbatim from rust/tests/golden/schema.json (approvals plan request).
    let raw = r#"{
  "name": "spender",
  "required": true,
  "default": "",
  "description": "Spender address",
  "schema": {
    "type": "string",
    "format": "evm-address"
  }
}"#;
    let field: SchemaField = serde_json::from_str(raw).unwrap();
    assert_eq!(field.name, "spender");
    assert!(field.required);
    assert_eq!(field.default, Some(json!("")));
    assert_eq!(field.schema.format, "evm-address");
    let back = serde_json::to_string_pretty(&field).unwrap();
    assert_eq!(back, raw, "request field must round-trip byte-for-byte");
}

#[test]
fn wallet_signer_auth_block_round_trips() {
    // Shape taken verbatim from the golden schema fixture auth block.
    let raw = r#"[
  {
    "kind": "wallet",
    "env_vars": [
      "DEFI_OWS_TOKEN"
    ],
    "description": "Primary auth for wallet-backed execution (execution_backend=ows): set DEFI_OWS_TOKEN in the environment. Submit uses the persisted wallet_id and does not accept owner private keys."
  },
  {
    "kind": "signer",
    "env_vars": [
      "DEFI_PRIVATE_KEY",
      "DEFI_PRIVATE_KEY_FILE",
      "DEFI_KEYSTORE_PATH",
      "DEFI_KEYSTORE_PASSWORD",
      "DEFI_KEYSTORE_PASSWORD_FILE"
    ],
    "optional": true,
    "description": "Local signer auth for actions planned with --from-address: provide a local signer via --private-key or env/file/keystore inputs."
  }
]"#;
    let auth: Vec<AuthRequirement> = serde_json::from_str(raw).unwrap();
    assert_eq!(auth.len(), 2);
    assert_eq!(auth[0].kind, "wallet");
    assert!(auth[1].optional);
    assert_eq!(auth[1].env_vars.len(), 5);
    let back = serde_json::to_string_pretty(&auth).unwrap();
    assert_eq!(back, raw, "auth block must round-trip byte-for-byte");
}

#[test]
fn command_metadata_round_trips_and_matches_field_names() {
    let raw = r#"{
  "mutation": true,
  "input_modes": [
    "flags",
    "json"
  ],
  "input_constraints": [
    {
      "kind": "exactly_one_of",
      "fields": [
        "wallet",
        "from_address"
      ]
    }
  ]
}"#;
    let meta: CommandMetadata = serde_json::from_str(raw).unwrap();
    assert!(meta.mutation);
    assert_eq!(meta.input_modes, vec!["flags", "json"]);
    assert_eq!(meta.input_constraints.len(), 1);
    assert!(meta.request.is_none());
    let back = serde_json::to_string_pretty(&meta).unwrap();
    assert_eq!(back, raw);
}

// ---------------------------------------------------------------------------
// 5: infer_enum_values (port of inferEnumValues)
// ---------------------------------------------------------------------------

#[test]
fn infer_enum_pipe_form() {
    assert_eq!(
        infer_enum_values("Yield provider (aave|morpho)"),
        Some(vec!["aave".to_string(), "morpho".to_string()])
    );
}

#[test]
fn infer_enum_pipe_form_trims_whitespace_tokens() {
    assert_eq!(
        infer_enum_values("type (exact-input | exact-output)"),
        Some(vec!["exact-input".to_string(), "exact-output".to_string()])
    );
}

#[test]
fn infer_enum_key_value_comma_form_uses_keys() {
    assert_eq!(
        infer_enum_values("Position type (supply=lend, borrow=debt)"),
        Some(vec!["supply".to_string(), "borrow".to_string()])
    );
}

#[test]
fn infer_enum_none_when_no_parens() {
    assert_eq!(infer_enum_values("plain usage with no enum"), None);
}

#[test]
fn infer_enum_none_when_empty_body() {
    assert_eq!(infer_enum_values("usage ()"), None);
}

#[test]
fn infer_enum_none_when_no_separator() {
    // Parenthetical without `|` and without `k=v,` shape -> no enum.
    assert_eq!(infer_enum_values("limit (default)"), None);
}

#[test]
fn infer_enum_kv_form_skips_parts_without_equals() {
    // Go: `left, _, ok := strings.Cut(part, "="); if ok && left != ""`.
    // A comma part lacking `=` is skipped (Cut's `ok` is false). Rust ports this via
    // `split_once('=')` returning `None` for such parts. Here `bare` has no `=` and is
    // dropped; `min=1`/`max=100` contribute their keys.
    assert_eq!(
        infer_enum_values("limit (min=1, bare, max=100)"),
        Some(vec!["min".to_string(), "max".to_string()])
    );
}

#[test]
fn infer_enum_kv_form_all_keys_empty_yields_none() {
    // Body has `=` and `,` (entering the k=v branch) but every left-hand key is empty
    // after sanitization, so the branch produces no values and the whole call returns None
    // (mirrors Go: empty `out` -> fall through -> nil).
    assert_eq!(infer_enum_values("x (=1, =2)"), None);
}

#[test]
fn infer_enum_pipe_takes_precedence_over_kv() {
    // Go checks the `|` branch first; a body containing both `|` and `=`/`,` is split on
    // `|` (each token sanitized to its first word).
    assert_eq!(
        infer_enum_values("mode (a=1 | b=2)"),
        Some(vec!["a=1".to_string(), "b=2".to_string()])
    );
}

#[test]
fn infer_enum_uses_outermost_parens() {
    // Go uses Index('(') (first) and LastIndex(')') (last), so nested parens are captured
    // wholesale into `body`. With `a|b` inside, the pipe branch applies and sanitize keeps
    // the first whitespace word of each token, stripping a trailing `)`.
    assert_eq!(
        infer_enum_values("opt (a|b (note))"),
        Some(vec!["a".to_string(), "b".to_string()])
    );
}

// ---------------------------------------------------------------------------
// 6: sanitize_enum_value (port of sanitizeEnumValue)
// ---------------------------------------------------------------------------

#[test]
fn sanitize_enum_takes_first_word() {
    assert_eq!(sanitize_enum_value("  aave protocol  "), "aave");
}

#[test]
fn sanitize_enum_strips_trailing_punctuation() {
    assert_eq!(sanitize_enum_value("morpho,"), "morpho");
    assert_eq!(sanitize_enum_value("exact-output)"), "exact-output");
    assert_eq!(sanitize_enum_value("kamino."), "kamino");
}

#[test]
fn sanitize_enum_empty_input() {
    assert_eq!(sanitize_enum_value("   "), "");
    assert_eq!(sanitize_enum_value(""), "");
}

// ---------------------------------------------------------------------------
// 7: split_schema_enum (port of splitSchemaEnum)
// ---------------------------------------------------------------------------

#[test]
fn split_schema_enum_trims_and_drops_empties() {
    assert_eq!(
        split_schema_enum(" aave , morpho ,, kamino "),
        vec![
            "aave".to_string(),
            "morpho".to_string(),
            "kamino".to_string()
        ]
    );
}

#[test]
fn split_schema_enum_empty() {
    assert!(split_schema_enum("").is_empty());
    assert!(split_schema_enum("  ,  , ").is_empty());
}

// ---------------------------------------------------------------------------
// 8: parse_string_slice_default (port of parseStringSliceDefault)
// ---------------------------------------------------------------------------

#[test]
fn parse_string_slice_default_empty_forms() {
    assert!(parse_string_slice_default("").is_empty());
    assert!(parse_string_slice_default("[]").is_empty());
    assert!(parse_string_slice_default("   ").is_empty());
}

#[test]
fn parse_string_slice_default_bracketed() {
    assert_eq!(
        parse_string_slice_default("[aave, morpho,kamino]"),
        vec![
            "aave".to_string(),
            "morpho".to_string(),
            "kamino".to_string()
        ]
    );
}

#[test]
fn parse_string_slice_default_unbracketed_and_drops_empties() {
    assert_eq!(
        parse_string_slice_default("aave,,morpho, "),
        vec!["aave".to_string(), "morpho".to_string()]
    );
}

// ---------------------------------------------------------------------------
// 9: full golden-fixture round-trip (primary contract oracle)
// ---------------------------------------------------------------------------

#[test]
fn full_golden_schema_data_node_round_trips_order_preserving() {
    // Load the real Go `defi schema` capture and round-trip its entire `data` node
    // (the full command tree) through the typed model. This is the strongest parity
    // assertion available to this L0 crate: the rendered envelope/byte-stable golden
    // test belongs to defi-app, but the serde *data model* must losslessly represent
    // every node the Go binary emits.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/golden/schema.json"
    );
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read golden schema fixture {path}: {e}"));
    let envelope: serde_json::Value =
        serde_json::from_str(&src).expect("golden schema.json must be valid JSON");
    let data = envelope
        .get("data")
        .cloned()
        .expect("golden schema envelope must contain `data`");

    // Sanity: the fixture must actually exercise the constructs we care about, otherwise
    // a trivially-shaped fixture could let an incomplete model pass.
    let data_text = serde_json::to_string(&data).unwrap();
    for token in [
        "\"items\"",
        "\"additional_properties\"",
        "\"when\"",
        "\"input_constraints\"",
        "\"response\"",
        "\"request\"",
        "\"enum\"",
        "\"use\"",
        "\"subcommands\"",
    ] {
        assert!(
            data_text.contains(token),
            "golden fixture is missing {token}; round-trip would be too weak to be meaningful"
        );
    }

    // Deserialize into the typed model. Any unrepresentable / renamed / missing field in
    // the model would surface here (deny_unknown_fields is not set, so the order-sensitive
    // equality below is what actually catches dropped/renamed/reordered keys).
    let cmd: CommandSchema =
        serde_json::from_value(data.clone()).expect("data node must deserialize into CommandSchema");

    // Re-serialize and compare as order-preserving JSON values. `serde_json/preserve_order`
    // is enabled workspace-wide, so `Value`'s object maps retain key order: this comparison
    // fails on ANY field reordering, rename, drop, or omitempty mismatch.
    let reserialized = serde_json::to_value(&cmd).expect("CommandSchema must serialize");
    assert_eq!(
        reserialized, data,
        "typed model must reproduce the full golden schema data node order-for-order"
    );

    // Belt-and-suspenders: pretty (2-space) re-render of the model parses back to the same
    // structure, confirming the indent contract is compatible with the model.
    let pretty = serde_json::to_string_pretty(&cmd).expect("pretty serialize");
    let reparsed: serde_json::Value = serde_json::from_str(&pretty).unwrap();
    assert_eq!(reparsed, data);
}
