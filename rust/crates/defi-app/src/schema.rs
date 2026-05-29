//! `schema` command group handler.
//!
//! Go source: `internal/app/runner.go::newSchemaCommand` plus the
//! cobra-coupled tree walk in `internal/schema/schema.go`
//! (`Build`/`serialize`/`collectFlags`).
//!
//! `schema [command path]` is a deterministic, offline, **metadata-only**
//! command: it walks the CLI command tree and emits a machine-readable
//! [`defi_schema::CommandSchema`] document as the `data` of a success envelope
//! (`meta.cache.status == "bypass"`). The whole-tree output is the golden
//! fixture `rust/tests/golden/schema.json`.
//!
//! ## Idiomatic-Rust shape
//!
//! Go's `schema.Build` walks a live `*cobra.Command` tree, reading flags via
//! `pflag` reflection and metadata from cobra annotations. Rust's `clap` does
//! not expose an equivalent stable introspection surface, so this module owns a
//! small clap-independent command-tree model — [`CommandNode`] / [`FlagSpec`] —
//! and the pure tree-walk ([`build`]/[`serialize`]/[`collect_flags`]) over it.
//!
//! The model is populated when the CLI command tree is wired (runner /
//! integration phase); this module owns the *algorithm* and the two
//! contract-bearing leaf descriptors it can build standalone today
//! ([`version_node`], [`schema_node`]) plus the persistent root flag set
//! ([`root_persistent_flags`]). The whole-tree golden parity is integration
//! work; the per-node parity for `version` / `schema` is asserted here against
//! the golden `schema.json` subtree.
//!
//! Contract details preserved from the Go reference:
//!   * **Flag ordering is alphabetical by name** (cobra `FlagSet.VisitAll`
//!     sorts), regardless of inherited/local scope.
//!   * `help` and hidden flags are dropped; hidden subcommands are dropped.
//!   * A flag's `scope` is `"inherited"` if it came from an ancestor's
//!     persistent flags, else `"local"`.
//!   * `default` carries the flag's typed default (bool/int/string/…); an empty
//!     string / false / etc. is still emitted for the form fields that are not
//!     `omitempty` (only `default` itself is `omitempty`, dropped when null).

use defi_errors::{Code, Error};
use defi_model::{CacheStatus, Envelope};
use defi_schema::{CommandMetadata, CommandSchema, FlagMetadata, FlagSchema};
use serde_json::Value;

/// The scope of a flag within a command node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagScope {
    /// A flag declared on this command (cobra "local").
    Local,
    /// A flag inherited from an ancestor's persistent flags.
    Inherited,
}

impl FlagScope {
    /// The wire string (`"local"` / `"inherited"`) used in the schema document.
    fn as_str(self) -> &'static str {
        match self {
            FlagScope::Local => "local",
            FlagScope::Inherited => "inherited",
        }
    }
}

/// A clap-independent flag descriptor (the data `collectFlags` reads off a
/// `pflag.Flag`).
///
/// `default` is the typed default value the schema emits; `None` is the Go
/// `nil` default (omitted via `omitempty`). `metadata` carries the
/// required/enum/format hints set out-of-band in Go via annotations.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagSpec {
    /// Flag long name (e.g. `"select"`).
    pub name: String,
    /// Single-char shorthand, empty when none.
    pub shorthand: String,
    /// pflag value type string (`"bool"`, `"int"`, `"string"`, …).
    pub type_name: String,
    /// Usage text.
    pub usage: String,
    /// Typed default value, or `None` to omit.
    pub default: Option<Value>,
    /// Whether the flag is hidden (dropped from the schema).
    pub hidden: bool,
    /// Out-of-band metadata (required / enum / format).
    pub metadata: FlagMetadata,
}

impl FlagSpec {
    /// A bool flag with the given default (the common persistent-flag shape).
    fn boolean(name: &str, usage: &str, default: bool) -> Self {
        FlagSpec {
            name: name.to_string(),
            shorthand: String::new(),
            type_name: "bool".to_string(),
            usage: usage.to_string(),
            default: Some(Value::Bool(default)),
            hidden: false,
            metadata: FlagMetadata::default(),
        }
    }

    /// A string flag with the given default.
    fn string(name: &str, usage: &str, default: &str) -> Self {
        FlagSpec {
            name: name.to_string(),
            shorthand: String::new(),
            type_name: "string".to_string(),
            usage: usage.to_string(),
            default: Some(Value::String(default.to_string())),
            hidden: false,
            metadata: FlagMetadata::default(),
        }
    }

    /// An int flag with the given default.
    fn integer(name: &str, usage: &str, default: i64) -> Self {
        FlagSpec {
            name: name.to_string(),
            shorthand: String::new(),
            type_name: "int".to_string(),
            usage: usage.to_string(),
            default: Some(Value::Number(default.into())),
            hidden: false,
            metadata: FlagMetadata::default(),
        }
    }

    /// Attach a `format` hint (builder-style).
    fn with_format(mut self, format: &str) -> Self {
        self.metadata.format = format.to_string();
        self
    }
}

/// A clap-independent command-tree node (the data `serialize` reads off a
/// `*cobra.Command`).
///
/// `local_flags` are this command's own flags; the persistent flags inherited
/// from ancestors are passed down the walk and merged at each node (sorted +
/// scoped) by [`collect_flags`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommandNode {
    /// The command's `Name()` (the first token of `use`), used for path walking.
    pub name: String,
    /// The cobra `Use` string (may carry an args spec, e.g. `"schema [command path]"`).
    pub r#use: String,
    /// Short description.
    pub short: String,
    /// Command aliases.
    pub aliases: Vec<String>,
    /// Whether the command is hidden (dropped from a parent's subcommands).
    pub hidden: bool,
    /// Out-of-band command metadata (mutation / auth / request / response / …).
    pub metadata: CommandMetadata,
    /// This command's own (non-persistent) flags.
    pub local_flags: Vec<FlagSpec>,
    /// Persistent flags this command contributes to its descendants.
    pub persistent_flags: Vec<FlagSpec>,
    /// Child commands.
    pub subcommands: Vec<CommandNode>,
}

impl CommandNode {
    /// A leaf command node with a name, use, and short description.
    pub fn leaf(name: &str, r#use: &str, short: &str) -> Self {
        CommandNode {
            name: name.to_string(),
            r#use: r#use.to_string(),
            short: short.to_string(),
            ..Default::default()
        }
    }
}

/// The root command's persistent flags (mirrors `newRootCommand`'s
/// `PersistentFlags()` block in `internal/app/runner.go`). These are inherited
/// by every subcommand. Returned in declaration order; the schema walk sorts
/// them by name where they surface.
pub fn root_persistent_flags() -> Vec<FlagSpec> {
    vec![
        FlagSpec::boolean("json", "Output JSON (default)", false),
        FlagSpec::boolean("plain", "Output plain text", false),
        FlagSpec::string("select", "Select fields from data (comma-separated)", ""),
        FlagSpec::boolean("results-only", "Output only data payload", false),
        FlagSpec::string(
            "enable-commands",
            "Allowlist command paths (comma-separated)",
            "",
        ),
        FlagSpec::boolean("strict", "Fail on partial results", false),
        FlagSpec::string("timeout", "Provider request timeout", ""),
        FlagSpec::integer("retries", "Retries per provider request", -1),
        FlagSpec::string(
            "max-stale",
            "Maximum stale fallback window after TTL expiry",
            "",
        ),
        FlagSpec::boolean("no-stale", "Reject stale cache entries", false),
        FlagSpec::boolean("no-cache", "Disable cache reads and writes", false),
        FlagSpec::string("config", "Path to config file", "").with_format("path"),
    ]
}

/// Build the `version` command node (mirrors `newVersionCommand`): a leaf with a
/// single local `--long` bool flag.
pub fn version_node() -> CommandNode {
    CommandNode {
        local_flags: vec![FlagSpec::boolean(
            "long",
            "Print extended build metadata",
            false,
        )],
        ..CommandNode::leaf("version", "version", "Print CLI version")
    }
}

/// Build the `schema` command node (mirrors `newSchemaCommand`): a leaf whose
/// metadata carries a `response` `TypeSchema` describing the schema document.
pub fn schema_node() -> CommandNode {
    CommandNode {
        metadata: CommandMetadata {
            response: Some(defi_schema::TypeSchema {
                r#type: "object".to_string(),
                description: "Machine-readable command schema document".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..CommandNode::leaf(
            "schema",
            "schema [command path]",
            "Print machine-readable command schema",
        )
    }
}

/// Walk the command tree from `root`, resolving `command_path` (space-separated
/// tokens) to a node, and serialize it (mirrors `schema.Build`).
///
/// An empty `command_path` serializes the root. Each path token must match a
/// (non-hidden or hidden — Go matches all children) child's `name` or one of its
/// `aliases`; an unresolved token is a [`Code::Usage`] error
/// (`"command not found: <path>"`), matching the Go `clierr.Wrap(CodeUsage, …)`
/// at the call site.
///
/// `root_inherited` is the set of persistent flags already in scope at `root`
/// (normally empty for the true root; the root contributes its own persistent
/// flags to its descendants).
pub fn build(
    root: &CommandNode,
    command_path: &str,
    root_inherited: &[FlagSpec],
) -> Result<CommandSchema, Error> {
    let mut node = root;
    let mut inherited: Vec<FlagSpec> = root_inherited.to_vec();
    // Path of resolved command names (`"defi"` + each matched token), used to
    // compute each node's `path`.
    let mut name_path: Vec<String> = vec![root.name.clone()];

    if !command_path.trim().is_empty() {
        for token in command_path.split_whitespace() {
            // The current node's persistent flags become inherited for its
            // children as we descend.
            let next_inherited = merge_persistent(&inherited, &node.persistent_flags);
            let found = node
                .subcommands
                .iter()
                .find(|c| c.name == token || c.aliases.iter().any(|a| a == token));
            match found {
                Some(child) => {
                    inherited = next_inherited;
                    node = child;
                    name_path.push(child.name.clone());
                }
                None => {
                    return Err(Error::new(
                        Code::Usage,
                        format!("command not found: {command_path}"),
                    ));
                }
            }
        }
    }

    Ok(serialize(node, &inherited, &name_path))
}

/// Serialize a single command node plus its (non-hidden) subcommands (mirrors
/// `schema.serialize`).
///
/// `inherited` is the set of persistent flags in scope from ancestors (NOT
/// including this node's own persistent flags); `name_path` is the resolved
/// command-name path used to compute `path`.
fn serialize(node: &CommandNode, inherited: &[FlagSpec], name_path: &[String]) -> CommandSchema {
    let meta = &node.metadata;
    let mut schema = CommandSchema {
        path: name_path.join(" "),
        r#use: node.r#use.clone(),
        short: node.short.clone(),
        aliases: node.aliases.clone(),
        mutation: meta.mutation,
        input_modes: meta.input_modes.clone(),
        input_constraints: meta.input_constraints.clone(),
        auth: meta.auth.clone(),
        request: meta.request.clone(),
        response: meta.response.clone(),
        flags: collect_flags(node, inherited),
        subcommands: Vec::new(),
    };

    // This node's persistent flags are inherited by its children.
    let child_inherited = merge_persistent(inherited, &node.persistent_flags);
    for sub in &node.subcommands {
        if sub.hidden {
            continue;
        }
        let mut child_path = name_path.to_vec();
        child_path.push(sub.name.clone());
        schema
            .subcommands
            .push(serialize(sub, &child_inherited, &child_path));
    }

    schema
}

/// Collect the schema flags for a node (mirrors `schema.collectFlags`).
///
/// Merges the node's local flags with the inherited persistent flags, drops
/// hidden + `help`, sorts by name (cobra `VisitAll` ordering), tags each with
/// its scope, and emits a [`FlagSchema`] per flag (with merged required/enum/
/// format metadata). When a local flag shadows an inherited one by name, the
/// local definition wins (it is the effective flag on this command).
fn collect_flags(node: &CommandNode, inherited: &[FlagSpec]) -> Vec<FlagSchema> {
    use std::collections::BTreeMap;

    // BTreeMap keeps the deterministic alphabetical-by-name order cobra produces
    // and deduplicates by flag name (local shadows inherited).
    let mut effective: BTreeMap<String, (FlagSpec, FlagScope)> = BTreeMap::new();
    for flag in inherited {
        if flag.hidden || flag.name == "help" {
            continue;
        }
        effective.insert(flag.name.clone(), (flag.clone(), FlagScope::Inherited));
    }
    for flag in &node.local_flags {
        if flag.hidden || flag.name == "help" {
            continue;
        }
        effective.insert(flag.name.clone(), (flag.clone(), FlagScope::Local));
    }

    effective
        .into_values()
        .map(|(flag, scope)| {
            let meta = merge_flag_metadata(&flag);
            FlagSchema {
                name: flag.name,
                shorthand: flag.shorthand,
                r#type: flag.type_name,
                usage: flag.usage,
                default: flag.default,
                required: meta.required,
                enum_values: meta.enum_values,
                format: meta.format,
                scope: scope.as_str().to_string(),
            }
        })
        .collect()
}

/// Merge a flag's explicit metadata with the enum inferred from its usage string
/// (mirrors `schema.MergedFlagMetadata`): an explicit `enum` wins; otherwise an
/// enum is inferred from the usage parenthetical (`"… (a|b)"`).
fn merge_flag_metadata(flag: &FlagSpec) -> FlagMetadata {
    let mut meta = flag.metadata.clone();
    if meta.enum_values.is_empty() {
        if let Some(inferred) = defi_schema::infer_enum_values(&flag.usage) {
            meta.enum_values = inferred;
        }
    }
    meta
}

/// Merge an ancestor inherited-flag set with a node's persistent flags. A
/// persistent flag with the same name as an existing inherited flag replaces it
/// (the nearer ancestor's definition wins, matching cobra's flag resolution).
fn merge_persistent(inherited: &[FlagSpec], persistent: &[FlagSpec]) -> Vec<FlagSpec> {
    if persistent.is_empty() {
        return inherited.to_vec();
    }
    let mut out: Vec<FlagSpec> = Vec::with_capacity(inherited.len() + persistent.len());
    for flag in inherited {
        if persistent.iter().any(|p| p.name == flag.name) {
            continue;
        }
        out.push(flag.clone());
    }
    out.extend(persistent.iter().cloned());
    out
}

/// Handle `schema [command path]`: build the schema document for `command_path`
/// over `root` and wrap it in a success envelope (cache bypassed).
///
/// Mirrors the Go `newSchemaCommand` handler: `schema.Build(root, path)` then
/// `emitSuccess(..., data, nil, cacheMetaBypass(), nil, false)` with command
/// `"schema"`. A failed build surfaces as a [`Code::Usage`] error.
pub fn run(
    root: &CommandNode,
    command_path: &str,
    root_inherited: &[FlagSpec],
) -> Result<Envelope, Error> {
    let document = build(root, command_path, root_inherited)?;
    let data = serde_json::to_value(&document)
        .map_err(|e| Error::wrap(Code::Internal, "serialize schema", e))?;
    Ok(Envelope::success(
        "schema",
        data,
        Vec::new(),
        CacheStatus::bypass(),
        Vec::new(),
        false,
    ))
}

#[cfg(test)]
mod tests {
    //! # Success criteria — `defi-app::schema` (Go: `internal/schema/schema.go`
    //! + `internal/app/runner.go::newSchemaCommand`)
    //!
    //! `schema` is a deterministic, offline, **metadata-only** command. It walks
    //! the CLI command tree and emits a [`defi_schema::CommandSchema`] document as
    //! the `data` of a success envelope (`cache.status == "bypass"`). The Rust
    //! port is "correct" iff it preserves the tree-walk contract and the
    //! per-node parity with the Go golden `schema.json`. Criteria asserted below
    //! (NOT Go internals — the serde data model + clap-free string helpers live
    //! in `defi-schema`):
    //!
    //!  S1. **Path resolution.** [`build`] resolves a space-separated
    //!      `command_path` to a node by matching each token against a child's
    //!      `name` or `aliases`; the resulting `path` is the resolved name chain
    //!      joined by spaces (`"defi yield plan"`). An empty path serializes the
    //!      root. (Go `Build` walk.)
    //!  S2. **Unknown path → usage error.** An unresolved token yields
    //!      [`Code::Usage`] with `"command not found: <path>"`. (Go `Build` error
    //!      → `clierr.Wrap(CodeUsage, …)`.)
    //!  S3. **Flag scope.** Persistent flags inherited from ancestors are tagged
    //!      `scope == "inherited"`; a command's own flags are `"local"`. (Go
    //!      `collectFlags` inherited-set check.)
    //!  S4. **Alphabetical flag order.** Flags within a node are sorted by name
    //!      regardless of scope — a local `--long` sorts between inherited `json`
    //!      and `max-stale`. (cobra `FlagSet.VisitAll` ordering.)
    //!  S5. **`help` + hidden dropped.** A `help` flag and any hidden flag are
    //!      excluded; hidden subcommands are excluded. (Go `collectFlags` /
    //!      `serialize`.)
    //!  S6. **Metadata propagation.** A node's `mutation` / `input_modes` /
    //!      `input_constraints` / `auth` / `request` / `response` flow into the
    //!      serialized node. (Go `serialize` from `CommandMetadataFor`.)
    //!  S7. **Enum inference.** A flag whose usage carries a `"(a|b)"`
    //!      parenthetical and no explicit enum gets `enum == [a, b]`; an explicit
    //!      enum wins. (Go `MergedFlagMetadata` → `inferEnumValues`.)
    //!  S8. **`version` node golden parity.** Serializing [`version_node`] under
    //!      the root persistent flags reproduces the `version` subtree of the Go
    //!      golden `schema.json` byte-for-byte (path, use, short, flags, scopes,
    //!      defaults, the local `--long` flag, alphabetical order).
    //!  S9. **`schema` node golden parity.** Serializing [`schema_node`]
    //!      reproduces the `schema [command path]` subtree of the golden, incl.
    //!      its `response` `TypeSchema` and the inherited persistent flag set.
    //! S10. **`run` envelope shape.** [`run`] returns a success envelope with
    //!      `meta.command == "schema"`, `cache.status == "bypass"`, `version ==
    //!      "v1"`, no providers/warnings, `partial == false`, and `data` equal to
    //!      the serialized document.
    //! S11. **Cache bypass** (metadata route — spec §2.5): `schema` bypasses the
    //!      cache (`runner::should_open_cache("schema") == false`).
    //!
    //! Skipped (owned elsewhere / Go-only):
    //!   * The whole-tree golden parity (`schema.json` in full) is integration
    //!     work — it needs the complete clap command tree, populated at runner
    //!     wiring. We assert per-node parity for `version`/`schema` here.
    //!   * `SchemaFromType` / `SchemaFromFlagBindings` (Go runtime reflection) do
    //!     not port to Rust; request/response schemas are built declaratively at
    //!     wiring time. The serde data model + string helpers are tested in
    //!     `defi-schema`.

    use super::*;
    use serde_json::json;

    const GOLDEN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden");

    /// The golden `schema.json` `data` object (the root `CommandSchema`).
    fn golden_data() -> Value {
        let path = format!("{GOLDEN_DIR}/schema.json");
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read golden: {e}"));
        let env: Value = serde_json::from_str(&raw).expect("parse golden schema envelope");
        env.get("data").expect("golden data").clone()
    }

    /// The golden subtree node whose `use` matches `use_str`.
    fn golden_subcommand(use_str: &str) -> Value {
        let data = golden_data();
        data.get("subcommands")
            .and_then(Value::as_array)
            .expect("golden subcommands")
            .iter()
            .find(|s| s.get("use").and_then(Value::as_str) == Some(use_str))
            .unwrap_or_else(|| panic!("golden subcommand {use_str} not found"))
            .clone()
    }

    /// A minimal root node with the real persistent flags + the two leaves this
    /// module owns, for build-walk tests.
    fn test_root() -> CommandNode {
        CommandNode {
            name: "defi".to_string(),
            r#use: "defi".to_string(),
            short: "Agent-first DeFi retrieval CLI".to_string(),
            persistent_flags: root_persistent_flags(),
            subcommands: vec![schema_node(), version_node()],
            ..Default::default()
        }
    }

    // ----- S1: path resolution --------------------------------------------
    #[test]
    fn build_resolves_command_path() {
        let root = test_root();
        let doc = build(&root, "version", &[]).expect("resolve version");
        assert_eq!(doc.path, "defi version");
        assert_eq!(doc.r#use, "version");
        assert_eq!(doc.short, "Print CLI version");
    }

    #[test]
    fn build_empty_path_serializes_root() {
        let root = test_root();
        let doc = build(&root, "", &[]).expect("serialize root");
        assert_eq!(doc.path, "defi");
        assert_eq!(doc.r#use, "defi");
        // root has no local flags; its persistent flags surface on its children,
        // not on itself.
        assert!(doc.flags.is_empty());
        // both leaves present (order = declaration order, hidden dropped).
        let subs: Vec<&str> = doc.subcommands.iter().map(|s| s.r#use.as_str()).collect();
        assert_eq!(subs, vec!["schema [command path]", "version"]);
    }

    #[test]
    fn build_resolves_via_alias() {
        let mut root = test_root();
        root.subcommands[1].aliases = vec!["ver".to_string()];
        let doc = build(&root, "ver", &[]).expect("resolve via alias");
        assert_eq!(doc.path, "defi version");
    }

    // ----- S2: unknown path -> usage error --------------------------------
    #[test]
    fn build_unknown_path_is_usage_error() {
        let root = test_root();
        let err = build(&root, "frobnicate", &[]).expect_err("unknown command rejected");
        assert_eq!(err.code, Code::Usage);
        assert!(
            err.to_string().contains("command not found: frobnicate"),
            "got: {err}"
        );
    }

    // ----- S3 & S4: flag scope + alphabetical order -----------------------
    #[test]
    fn version_flags_are_scoped_and_alphabetical() {
        let root = test_root();
        let doc = build(&root, "version", &[]).expect("version node");
        let names: Vec<&str> = doc.flags.iter().map(|f| f.name.as_str()).collect();
        // Alphabetical by name; `long` (local) sorts between json and max-stale.
        assert_eq!(
            names,
            vec![
                "config",
                "enable-commands",
                "json",
                "long",
                "max-stale",
                "no-cache",
                "no-stale",
                "plain",
                "results-only",
                "retries",
                "select",
                "strict",
                "timeout",
            ]
        );
        // `long` is local; everything else is inherited.
        for f in &doc.flags {
            let want = if f.name == "long" {
                "local"
            } else {
                "inherited"
            };
            assert_eq!(f.scope, want, "flag {} scope", f.name);
        }
    }

    // ----- S5: help + hidden dropped --------------------------------------
    #[test]
    fn collect_flags_drops_help_and_hidden() {
        let mut node = CommandNode::leaf("x", "x", "x cmd");
        node.local_flags = vec![
            FlagSpec::boolean("visible", "shown", false),
            FlagSpec::boolean("help", "auto help", false),
            FlagSpec {
                hidden: true,
                ..FlagSpec::boolean("secret", "hidden", false)
            },
        ];
        let flags = collect_flags(&node, &[]);
        let names: Vec<&str> = flags.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["visible"], "help + hidden flags dropped");
    }

    #[test]
    fn serialize_drops_hidden_subcommands() {
        let mut root = test_root();
        root.subcommands.push(CommandNode {
            hidden: true,
            ..CommandNode::leaf("secret", "secret", "hidden cmd")
        });
        let doc = build(&root, "", &[]).expect("serialize root");
        assert!(
            !doc.subcommands.iter().any(|s| s.r#use == "secret"),
            "hidden subcommand must be dropped"
        );
    }

    // ----- S6: metadata propagation ---------------------------------------
    #[test]
    fn serialize_propagates_command_metadata() {
        let doc = build(&test_root(), "schema", &[]).expect("schema node");
        let response = doc.response.as_ref().expect("response metadata present");
        assert_eq!(response.r#type, "object");
        assert_eq!(
            response.description,
            "Machine-readable command schema document"
        );
    }

    #[test]
    fn serialize_propagates_mutation_and_constraints() {
        let mut node = CommandNode::leaf("plan", "plan", "create a plan");
        node.metadata = CommandMetadata {
            mutation: true,
            input_modes: vec!["flags".to_string(), "json".to_string()],
            input_constraints: vec![defi_schema::InputConstraint {
                kind: "exactly_one_of".to_string(),
                fields: vec!["wallet".to_string(), "from_address".to_string()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let root = CommandNode {
            name: "defi".to_string(),
            r#use: "defi".to_string(),
            subcommands: vec![node],
            ..Default::default()
        };
        let doc = build(&root, "plan", &[]).expect("plan node");
        assert!(doc.mutation);
        assert_eq!(doc.input_modes, vec!["flags", "json"]);
        assert_eq!(doc.input_constraints.len(), 1);
        assert_eq!(doc.input_constraints[0].kind, "exactly_one_of");
        assert_eq!(
            doc.input_constraints[0].fields,
            vec!["wallet", "from_address"]
        );
    }

    // ----- S7: enum inference ---------------------------------------------
    #[test]
    fn collect_flags_infers_enum_from_usage_parenthetical() {
        let mut node = CommandNode::leaf("x", "x", "x cmd");
        node.local_flags = vec![FlagSpec::string(
            "provider",
            "Yield provider (aave|morpho)",
            "",
        )];
        let flags = collect_flags(&node, &[]);
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].enum_values, vec!["aave", "morpho"]);
    }

    #[test]
    fn collect_flags_explicit_enum_wins_over_usage() {
        let mut node = CommandNode::leaf("x", "x", "x cmd");
        let mut flag = FlagSpec::string("provider", "Yield provider (aave|morpho)", "");
        flag.metadata.enum_values = vec!["custom".to_string()];
        node.local_flags = vec![flag];
        let flags = collect_flags(&node, &[]);
        assert_eq!(
            flags[0].enum_values,
            vec!["custom"],
            "explicit enum metadata wins over inferred"
        );
    }

    // ----- S8: version node golden parity ---------------------------------
    #[test]
    fn version_node_matches_go_golden_subtree() {
        let doc = build(&test_root(), "version", &[]).expect("version node");
        let got = serde_json::to_value(&doc).expect("serialize version node");
        assert_eq!(
            got,
            golden_subcommand("version"),
            "version schema node must match the Go golden subtree byte-for-byte"
        );
    }

    // ----- S9: schema node golden parity ----------------------------------
    #[test]
    fn schema_node_matches_go_golden_subtree() {
        let doc = build(&test_root(), "schema", &[]).expect("schema node");
        let got = serde_json::to_value(&doc).expect("serialize schema node");
        assert_eq!(
            got,
            golden_subcommand("schema [command path]"),
            "schema schema node must match the Go golden subtree byte-for-byte"
        );
    }

    // ----- S10: run envelope shape ----------------------------------------
    #[test]
    fn run_returns_bypass_success_envelope() {
        let root = test_root();
        let env = run(&root, "version", &[]).expect("run schema for version");
        assert!(env.success);
        assert!(env.error.is_none());
        assert_eq!(env.version, "v1");
        assert_eq!(env.meta.command, "schema");
        assert_eq!(env.meta.cache.status, "bypass");
        assert_eq!(env.meta.cache.age_ms, 0);
        assert!(!env.meta.cache.stale);
        assert!(env.meta.providers.is_empty());
        assert!(!env.meta.partial);
        assert!(env.warnings.is_empty());

        // data equals the serialized document.
        let doc = build(&root, "version", &[]).expect("doc");
        let data = env.data.as_ref().expect("data present");
        assert_eq!(data, &serde_json::to_value(&doc).expect("serialize doc"));
    }

    #[test]
    fn run_unknown_path_is_usage_error() {
        let err = run(&test_root(), "nope", &[]).expect_err("unknown path rejected");
        assert_eq!(err.code, Code::Usage);
    }

    // ----- S11: cache bypass ----------------------------------------------
    #[test]
    fn schema_bypasses_cache() {
        assert!(
            !crate::runner::should_open_cache("schema"),
            "schema must bypass cache"
        );
    }

    // ----- envelope JSON field order (defensive) --------------------------
    #[test]
    fn run_envelope_preserves_top_level_field_order() {
        let env = run(&test_root(), "", &[]).expect("run root schema");
        let rendered = env.to_pretty_json().expect("render envelope");
        let value: Value = serde_json::from_str(&rendered).expect("parse rendered");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["version", "success", "data", "error", "meta"]);
    }

    // ----- defensive: default value typing --------------------------------
    #[test]
    fn flag_defaults_carry_typed_values() {
        let doc = build(&test_root(), "version", &[]).expect("version node");
        let retries = doc
            .flags
            .iter()
            .find(|f| f.name == "retries")
            .expect("retries flag");
        assert_eq!(retries.default, Some(json!(-1)));
        let json_flag = doc
            .flags
            .iter()
            .find(|f| f.name == "json")
            .expect("json flag");
        assert_eq!(json_flag.default, Some(json!(false)));
        let select = doc
            .flags
            .iter()
            .find(|f| f.name == "select")
            .expect("select flag");
        assert_eq!(select.default, Some(json!("")));
    }
}
