//! Golden IR tests over the reference examples.
//!
//! Each example in `examples/` is compiled and compared byte-for-byte against a
//! checked-in IR document in `tests/golden-ir/`. These are the tests that make
//! "the same source always produces the same artifact" a maintained property
//! rather than an aspiration: any change to lowering shows up as a reviewable
//! diff in the golden file.
//!
//! To accept an intended change:
//!
//! ```text
//! INGOT_UPDATE_GOLDEN=1 cargo test -p ingot-compiler --test golden_ir
//! ```
//!
//! Review the resulting diff before committing it. An unexplained change here
//! means the compiled meaning of every existing agent moved.

use std::path::{Path, PathBuf};

use ingot_compiler::compile_path;
use ingot_diagnostics::ColorChoice;

/// Every example, with the agents it is expected to declare.
const EXAMPLES: &[(&str, &[&str])] = &[
    ("document-summarizer", &["DocumentSummarizer"]),
    ("research-agent", &["ResearchAgent"]),
    ("code-review-team", &["SecurityReviewer", "CodeReviewTeam"]),
    ("repo-digest", &["RepoDigest"]),
];

fn repo_root() -> PathBuf {
    // crates/ingot-compiler -> crates -> repository root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate must live two levels below the repository root")
        .to_path_buf()
}

fn updating() -> bool {
    std::env::var_os("INGOT_UPDATE_GOLDEN").is_some()
}

#[test]
fn examples_compile_without_errors() {
    for (example, _) in EXAMPLES {
        let entry = repo_root().join("examples").join(example).join("main.ing");
        let compilation = compile_path(&entry).expect("example source must be readable");
        assert!(
            !compilation.has_errors(),
            "example `{example}` must compile cleanly:\n{}",
            compilation.render_diagnostics(ColorChoice::Never)
        );
    }
}

#[test]
fn examples_declare_the_expected_agents() {
    for (example, expected) in EXAMPLES {
        let entry = repo_root().join("examples").join(example).join("main.ing");
        let compilation = compile_path(&entry).expect("example source must be readable");
        let names: Vec<String> = compilation
            .agents
            .iter()
            .map(|agent| {
                agent
                    .agent
                    .rsplit('.')
                    .next()
                    .unwrap_or(&agent.agent)
                    .to_string()
            })
            .collect();
        assert_eq!(&names, expected, "agent list changed for `{example}`");
    }
}

#[test]
fn ir_matches_the_golden_files() {
    let root = repo_root();
    let golden_dir = root.join("tests").join("golden-ir");
    if updating() {
        std::fs::create_dir_all(&golden_dir).expect("creating the golden directory");
    }

    let mut mismatches = Vec::new();

    for (example, _) in EXAMPLES {
        let entry = root.join("examples").join(example).join("main.ing");
        let compilation = compile_path(&entry).expect("example source must be readable");
        assert!(
            !compilation.has_errors(),
            "example `{example}` must compile cleanly:\n{}",
            compilation.render_diagnostics(ColorChoice::Never)
        );

        for agent in &compilation.agents {
            let short = agent.agent.rsplit('.').next().unwrap_or(&agent.agent);
            let golden = golden_dir.join(format!("{example}.{short}.ir.json"));
            let actual = agent.to_canonical_json();

            if updating() {
                std::fs::write(&golden, &actual).expect("writing the golden file");
                continue;
            }

            match std::fs::read_to_string(&golden) {
                Ok(expected) if expected == actual => {}
                Ok(expected) => mismatches.push(format!(
                    "{}\n{}",
                    golden.display(),
                    first_difference(&expected, &actual)
                )),
                Err(_) => mismatches.push(format!(
                    "{} is missing\nrun with INGOT_UPDATE_GOLDEN=1 to create it",
                    golden.display()
                )),
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "IR no longer matches the golden files:\n\n{}\n\n\
         If the change is intended, re-run with INGOT_UPDATE_GOLDEN=1 and review the diff.",
        mismatches.join("\n\n")
    );
}

#[test]
fn compiling_twice_produces_identical_bytes() {
    for (example, _) in EXAMPLES {
        let entry = repo_root().join("examples").join(example).join("main.ing");
        let first = compile_path(&entry).expect("readable");
        let second = compile_path(&entry).expect("readable");
        let left: Vec<String> = first
            .agents
            .iter()
            .map(|agent| agent.to_canonical_json())
            .collect();
        let right: Vec<String> = second
            .agents
            .iter()
            .map(|agent| agent.to_canonical_json())
            .collect();
        assert_eq!(left, right, "`{example}` is not deterministic");
    }
}

#[test]
fn golden_files_parse_back_into_the_ir_model() {
    if updating() {
        // The update run rewrites these files concurrently with this test.
        return;
    }
    let golden_dir = repo_root().join("tests").join("golden-ir");
    let Ok(entries) = std::fs::read_dir(&golden_dir) else {
        // The update run creates the directory; nothing to verify yet.
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("reading the golden file");
        let ir = ingot_ir::AgentIr::from_json(&text)
            .unwrap_or_else(|error| panic!("{} is not valid IR: {error}", path.display()));
        assert_eq!(
            ir.to_canonical_json(),
            text,
            "{} is not in canonical form",
            path.display()
        );
    }
}

/// The published schema and the Rust model must agree on the closed vocabularies.
///
/// Without this, the schema quietly becomes documentation of a past version.
#[test]
fn the_published_schema_lists_the_same_node_kinds_and_effects() {
    let path = repo_root()
        .join("specs")
        .join("ir")
        .join("agent-ir.schema.json");
    let text = std::fs::read_to_string(&path).expect("the IR schema must be readable");
    let schema: serde_json::Value =
        serde_json::from_str(&text).expect("the IR schema must be valid JSON");

    let kinds = schema["$defs"]["node"]["properties"]["kind"]["enum"]
        .as_array()
        .expect("the schema must enumerate node kinds");
    let mut schema_kinds: Vec<&str> = kinds
        .iter()
        .map(|kind| kind.as_str().expect("kinds are strings"))
        .collect();
    schema_kinds.sort_unstable();

    let mut model_kinds: Vec<&str> = ALL_NODE_KINDS.iter().map(|kind| kind.as_str()).collect();
    model_kinds.sort_unstable();
    assert_eq!(
        schema_kinds, model_kinds,
        "node kinds drifted between schema and model"
    );

    let effects = schema["$defs"]["effect"]["enum"]
        .as_array()
        .expect("the schema must enumerate effects");
    let schema_effects: Vec<&str> = effects
        .iter()
        .map(|effect| effect.as_str().expect("effects are strings"))
        .collect();
    let model_effects: Vec<&str> = ingot_types::Effect::all()
        .iter()
        .map(|effect| effect.as_str())
        .collect();
    assert_eq!(
        schema_effects, model_effects,
        "effects drifted between schema and model"
    );
}

use ingot_ir::NodeKind;

const ALL_NODE_KINDS: &[NodeKind] = &[
    NodeKind::LlmCall,
    NodeKind::ToolCall,
    NodeKind::AgentCall,
    NodeKind::Branch,
    NodeKind::Parallel,
    NodeKind::Loop,
    NodeKind::Approval,
    NodeKind::Verify,
    NodeKind::StateRead,
    NodeKind::StateWrite,
    NodeKind::ArtifactEmit,
    NodeKind::Checkpoint,
];

/// Report the first differing line, which is far easier to read than a full dump.
fn first_difference(expected: &str, actual: &str) -> String {
    for (index, (expected_line, actual_line)) in expected.lines().zip(actual.lines()).enumerate() {
        if expected_line != actual_line {
            return format!(
                "  first difference on line {}:\n    expected: {expected_line}\n    actual:   {actual_line}",
                index + 1
            );
        }
    }
    format!(
        "  identical for {} lines, then the length differs ({} expected vs {} actual)",
        expected.lines().count().min(actual.lines().count()),
        expected.lines().count(),
        actual.lines().count()
    )
}
