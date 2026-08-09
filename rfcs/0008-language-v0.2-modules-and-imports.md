# RFC-0008: Language 0.2 modules and imports

- Status: Draft
- Author(s): Heptapus Group
- Created: 2026-08-09
- Affects: language, compiler, CLI, editor tooling

## Problem

Language 0.1 makes every compilation unit one `.ing` file. That is pleasant for
the first agent, but it becomes friction as soon as a project grows past one
workflow. A realistic project wants declarations like this in one place:

```ingot
type search_result {
  title: string
  url: string
  snippet: string
}

tool web.search(query: string) -> search_result[] !network
```

Today each agent that searches the web must copy both declarations. Copying is
not just annoying; it creates drift. One file can update the tool's return type
or effect list while another keeps the old one, and the compiler can only judge
the file it was given.

The first Language 0.2 reuse feature should let a project share declarations
without adding a package manager, hidden build graph, or general-purpose module
system.

## Goals and non-goals

Goals:

- Let an entry source import shared `type`, `tool` and `verifier` declarations
  from another `.ing` file in the same project.
- Keep imports explicit enough that review shows exactly which names enter a
  file's scope.
- Preserve the current semantic model after import resolution: one checked
  program with one set of declarations.
- Keep `ingot check`, `ingot build`, `ingot fmt`, `ingot dev` and `ingot-lsp`
  able to reason over the same source graph.
- Close the copy-paste part of GAP-011 while leaving larger distribution and
  package-manager questions to a later RFC.

Non-goals:

- Importing agents. Shared sub-agents need a separate execution and packaging
  story, because they may carry their own policy, tools, state and output.
- Re-exporting imports.
- Wildcard imports.
- External package resolution, registries, semver ranges or lockfile changes.
- Optional values, unions, generics or pure helper functions. Those remain the
  later child RFCs tracked by issue #7.

## Proposed syntax

Common case:

```ingot
language 0.2
package heptapus.examples.research

import "./shared/web.ing" {
  type search_result
  tool web.search
}

agent ResearchAgent(topic: string) -> report<markdown> {
  tools {
    mcp web.search
  }

  policy {
    network allow ["api.search.example"]
  }

  flow {
    hits = call web.search(topic)
    emit report = ask<markdown>("Summarise ${topic}", context: hits)
  }
}
```

The imported file is ordinary Ingot source:

```ingot
language 0.2
package heptapus.shared.web

type search_result {
  title: string
  url: string
  snippet: string
}

tool web.search(query: string) -> search_result[] !network
```

Imports appear after `language` and optional `package`, before local
declarations:

```text
program        := language-decl package-decl? import-decl* item*
import-decl    := "import" string import-list
import-list    := "{" import-spec* "}"
import-spec    := import-kind import-name import-alias?
import-kind    := "type" | "tool" | "verifier"
import-name    := ident | dotted-name
import-alias   := "as" dotted-name
```

Rules:

- `import` is a new reserved keyword in Language 0.2.
- The import path is a string literal naming another `.ing` file.
- Import path strings must be plain strings with no `${...}` interpolation.
- The path is resolved relative to the importing file.
- The path must be relative, must stay inside the project root for manifest
  projects, must end in `.ing`, and must not contain `..`.
- For loose single-file compilation, imports may only resolve below the
  importing file's directory.
- Each imported file must start with `language 0.2`. Language 0.1 files remain
  single-file programs.
- Imported files may be library-only: they do not need to declare an agent. The
  complete entry compilation must still contain at least one agent.
- The imported file's `package` is metadata for diagnostics, docs and future
  packaging; it is not used to resolve the path in this RFC.
- `type` and `verifier` imports name an identifier unless an alias is supplied.
- `tool` imports name a dotted tool name unless an alias is supplied.
- Aliases introduce the alias into the importing file's scope. The alias must be
  valid for the imported kind: identifiers for `type` and `verifier`, dotted
  names for `tool`. The original name remains the name inside the defining file
  only.
- Imports are not transitive in source scope. If `main.ing` imports `a.ing` and
  `a.ing` imports `b.ing`, declarations from `b.ing` are available to `a.ing`
  while checking `a.ing`, but `main.ing` must import from `b.ing` explicitly to
  use those names.
- An import cycle is an error.

Duplicate handling follows the existing declaration discipline: a file may not
define or import two visible declarations of the same kind and name. Local
declarations do not silently shadow imports; the compiler reports a duplicate so
reviewers see the conflict.

## Static semantics

The compiler resolves the import graph before semantic analysis. After
resolution, imported declarations participate in the same checks as local
declarations:

- a type imported from another file may be used in fields, tool signatures,
  verifier signatures, prompt result types and output content checks;
- an imported tool may be granted in an agent's `tools` block and called in
  `flow`;
- an imported verifier may be used in `verify`;
- imported tool effects still contribute to the caller's effect set;
- policy remains local to the agent using the imported tool.

Doc comments attached to imported declarations travel with the declaration for
editor hover and generated docs. Diagnostic spans for imported declarations point
to the defining file. Diagnostics that describe a use point keep the importing
file as the primary span and may attach related information for the imported
declaration.

## IR semantics

Imports do not appear in Agent IR. They are compile-time source structure only.
The compiler lowers the resolved program exactly as if the imported declarations
had been written in the entry source, except that diagnostics and editor
metadata retain the original source file and byte spans.

No new IR node kinds, value forms or canonical JSON fields are required for this
RFC. A successful import-using program can still lower to Agent IR 0.1.

The source language version moves to `0.2`. Existing backends that refuse
unknown source language versions must report that refusal explicitly until they
opt in to Language 0.2. Once opted in, the reference interpreter and Python
backend consume the same IR shape they already understand.

## Target lowering

Reference interpreter:

- no runtime import loading;
- consumes the already-lowered Agent IR;
- enforces imported tool effects through the same artifact policy checks.

Python backend:

- emits the same self-contained Python shape as it does for copied declarations;
- reports no additional portability limitation when imports resolve;
- refuses only if the source language version or a later Language 0.2 feature is
  unsupported by that backend.

CLI:

- `ingot check` and `ingot build` load the import graph from the entry target;
- `ingot fmt` formats the entry file and, with a future flag, may format the
  whole import graph. The first implementation may format only the explicitly
  requested file but must parse import declarations and preserve canonical
  import layout;
- `ingot dev` watches the entry file, manifest and every imported source file;
- `ingot tools --propose` may emit declarations into a shared `.ing` module, but
  this RFC does not require it to write files automatically.

Editor tooling:

- diagnostics can span multiple files;
- completion includes explicitly imported declarations;
- hover/go-to-definition can jump from a use in the entry file to an imported
  declaration file.

## Security and policy impact

Imports add no runtime effects and cannot grant a capability. An imported tool
still needs:

- a `tools` grant in the agent that calls it;
- a policy rule allowing or requiring approval for its effects;
- a matching MCP route in the operator's manifest at run time.

The new risk is compile-time file access. To keep reviewable source boundaries,
Language 0.2 imports are project-local:

- absolute import paths are rejected;
- parent traversal is rejected;
- manifest projects cannot import outside the manifest root;
- loose source files cannot import outside the importing file's directory tree.

The compiler must never read a path only because a model, tool server or runtime
suggested it. Imports are source-authored and visible in review.

## Static bounds

Imports do not execute. They do not affect step counting, loop bounds, budget
checking or approval insertion except through the declarations they make
available. A call to an imported tool is counted and gated exactly like a call
to a local tool declaration.

## Compatibility

Existing Language 0.1 source is unchanged. A `language 0.1` file containing an
`import` declaration remains a syntax error.

Language 0.2 source using imports may lower to Agent IR 0.1 because imports are
erased after semantic analysis. The IR document's `language` field records the
source language as `0.2`; backends that support the structural IR but have not
opted into Language 0.2 must refuse with a portability diagnostic rather than
running anyway.

Migration is mechanical:

1. move copied shared declarations into a `.ing` file under the project;
2. update that file and the entry file to `language 0.2`;
3. add explicit imports to each entry file that used the copied declarations;
4. delete the copied declarations from the entry files;
5. run `ingot fmt`, `ingot check` and backend portability tests.

## Alternatives

**Do nothing.** Keeps the language tiny but forces serious projects to keep
copying declarations. That undermines the product loop: the editor can help with
one file, but the project still drifts across files.

**Dotted package imports instead of path imports.** This is attractive because
Ingot already has `package`, but it immediately raises source roots, package
lookup and external distribution questions. Path imports are less magical and
fit the existing manifest model. Dotted package resolution can be reconsidered
with M6 packaging and lockfiles.

**Wildcard imports.** They are convenient during authoring but make review
harder: a source diff no longer shows which declarations entered scope. The
first version keeps imports explicit.

**Import agents too.** This is useful, but an agent is not just a declaration.
It carries executable flow, policy, tools, memory and output. Sharing agents
should be specified with sub-agent packaging and policy composition in mind.

**A separate `module` declaration.** A new top-level module construct would make
library files explicit, but the existing `language` + optional `package` header
is enough. The entry compilation, not each file, is what must contain an agent.

## Conformance tests

- [ ] `modules_import_shared_type_and_tool`
- [ ] `imported_tool_still_requires_tool_grant`
- [ ] `imported_tool_still_requires_policy_rule`
- [ ] `imported_verifier_can_be_used_from_entry_flow`
- [ ] `imported_declaration_diagnostics_point_to_defining_file`
- [ ] `duplicate_local_and_imported_declarations_are_rejected`
- [ ] `imports_are_not_transitive_in_entry_scope`
- [ ] `import_cycle_is_rejected`
- [ ] `absolute_import_path_is_rejected`
- [ ] `parent_traversal_import_is_rejected`
- [ ] `loose_source_import_cannot_escape_its_directory_tree`
- [ ] `manifest_project_import_cannot_escape_project_root`
- [ ] `formatter_preserves_canonical_import_blocks`
- [ ] `dev_rechecks_when_imported_source_changes`
- [ ] `lsp_definition_jumps_to_imported_declaration`
- [ ] `imported_and_copied_sources_lower_to_identical_ir`
