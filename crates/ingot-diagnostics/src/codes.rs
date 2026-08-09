//! Stable diagnostic codes.
//!
//! Codes are part of the public contract: tooling greps for them, users search
//! for them and CI policies allow-list them. A code is never reused for a
//! different meaning. Removing a check means retiring its code, not recycling it.
//!
//! | Range     | Area                                        |
//! |-----------|---------------------------------------------|
//! | `ING1xxx` | Lexing and parsing                          |
//! | `ING2xxx` | Name resolution and declarations            |
//! | `ING3xxx` | Types                                       |
//! | `ING4xxx` | Effects, capabilities and policy            |
//! | `ING5xxx` | Budgets and static bounds                   |
//! | `ING6xxx` | Lowering and IR construction                |

// --- ING1xxx: lexing and parsing -----------------------------------------

pub const UNEXPECTED_CHARACTER: &str = "ING1001";
pub const UNTERMINATED_STRING: &str = "ING1002";
pub const UNTERMINATED_BLOCK_COMMENT: &str = "ING1003";
pub const INVALID_NUMBER: &str = "ING1004";
pub const UNTERMINATED_INTERPOLATION: &str = "ING1005";
pub const UNEXPECTED_TOKEN: &str = "ING1010";
pub const EXPECTED_TOKEN: &str = "ING1011";
pub const UNSUPPORTED_LANGUAGE_VERSION: &str = "ING1020";
pub const MISSING_LANGUAGE_DECLARATION: &str = "ING1021";

// --- ING2xxx: names and declarations -------------------------------------

pub const UNRESOLVED_NAME: &str = "ING2001";
pub const DUPLICATE_DECLARATION: &str = "ING2002";
pub const UNKNOWN_TYPE: &str = "ING2003";
pub const UNKNOWN_TOOL: &str = "ING2004";
pub const TOOL_NOT_GRANTED: &str = "ING2005";
pub const UNKNOWN_STATE_FIELD: &str = "ING2006";
pub const UNKNOWN_OUTPUT: &str = "ING2007";
pub const MISSING_FLOW_BLOCK: &str = "ING2008";
pub const UNRESOLVED_INTERPOLATION: &str = "ING2009";
pub const UNKNOWN_VERIFIER: &str = "ING2010";
pub const DUPLICATE_SECTION: &str = "ING2011";
pub const UNUSED_TOOL_GRANT: &str = "ING2012";
pub const NO_AGENT_DECLARED: &str = "ING2013";
pub const RECURSIVE_AGENT: &str = "ING2014";
pub const MISSING_OUTPUT_DECLARATION: &str = "ING2015";
pub const UNSUPPORTED_TRANSPORT: &str = "ING2016";
pub const UNSUPPORTED_MEMORY_LIFETIME: &str = "ING2017";
pub const IMPORT_RESOLUTION_ERROR: &str = "ING2018";
pub const FUNCTION_NOT_PURE: &str = "ING2019";

// --- ING3xxx: types -------------------------------------------------------

pub const TYPE_MISMATCH: &str = "ING3001";
pub const ARGUMENT_COUNT_MISMATCH: &str = "ING3002";
pub const UNKNOWN_ARGUMENT: &str = "ING3003";
pub const NOT_A_LIST: &str = "ING3004";
pub const PARALLEL_BODY_MUST_YIELD_VALUE: &str = "ING3005";
pub const CONDITION_NOT_BOOL: &str = "ING3006";
pub const EMIT_TYPE_MISMATCH: &str = "ING3007";
pub const INVALID_ARTIFACT_TYPE: &str = "ING3008";
pub const NOT_A_RECORD: &str = "ING3009";
pub const UNKNOWN_FIELD: &str = "ING3010";
pub const MISSING_ARGUMENT: &str = "ING3011";
pub const INVALID_OPERAND_TYPE: &str = "ING3012";

// --- ING4xxx: effects, capabilities and policy ---------------------------

pub const DENIED_CAPABILITY: &str = "ING4001";
pub const UNKNOWN_EFFECT: &str = "ING4002";
pub const APPROVAL_INSERTED: &str = "ING4003";
pub const UNKNOWN_POLICY_SUBJECT: &str = "ING4004";
pub const DUPLICATE_POLICY_RULE: &str = "ING4005";
pub const INVALID_POLICY_ACTION: &str = "ING4006";
pub const MISSING_POLICY_RULE: &str = "ING4007";
pub const UNKNOWN_MODEL_CAPABILITY: &str = "ING4008";

// --- ING5xxx: budgets and static bounds ----------------------------------

pub const UNBOUNDED_LOOP: &str = "ING5001";
pub const INVALID_BUDGET_VALUE: &str = "ING5002";
pub const UNKNOWN_BUDGET_KEY: &str = "ING5003";
pub const DUPLICATE_BUDGET_KEY: &str = "ING5004";
pub const MISSING_COST_CURRENCY: &str = "ING5005";
pub const STATIC_STEPS_EXCEED_BUDGET: &str = "ING5006";

// --- ING6xxx: lowering and IR --------------------------------------------

pub const OUTPUT_NEVER_EMITTED: &str = "ING6001";
pub const UNUSED_BINDING: &str = "ING6002";
pub const UNREACHABLE_STATEMENT: &str = "ING6003";
pub const OUTPUT_NOT_ON_ALL_PATHS: &str = "ING6004";
pub const INVALID_IN_PARALLEL: &str = "ING6005";

/// Long-form explanation shown by `ingot explain <CODE>`.
///
/// Every explanation states what the compiler saw, why it is rejected and the
/// concrete edit that fixes it.
pub fn explain(code: &str) -> Option<&'static str> {
    let text = match code.to_ascii_uppercase().as_str() {
        UNEXPECTED_CHARACTER => {
            "The lexer found a character that cannot start any Ingot token.\n\n\
             Check for stray punctuation, a smart quote pasted from a document \
             editor, or a missing string delimiter."
        }
        UNTERMINATED_STRING => {
            "A string literal reached the end of the line or file without a \
             closing double quote.\n\nIngot string literals are single-line. Use \
             \\n inside the literal for line breaks."
        }
        UNSUPPORTED_LANGUAGE_VERSION => {
            "The `language` declaration names a version this compiler does not \
             implement.\n\nEach source file declares the language version it was \
             written against, so the compiler never guesses semantics. Either \
             change the declaration or install a matching compiler."
        }
        MISSING_LANGUAGE_DECLARATION => {
            "The file does not start with a `language` declaration.\n\n\
             Add `language 0.1` as the first item. Pinning the version is what \
             lets the language evolve without silently changing the meaning of \
             existing agents."
        }
        UNRESOLVED_NAME => {
            "A name is used that is not an input, a binding, a state field, a \
             tool, a verifier or an agent in scope.\n\nBindings are visible only \
             after the statement that introduces them, and only inside the block \
             where they were introduced."
        }
        TOOL_NOT_GRANTED => {
            "The flow calls a declared tool that the agent's `tools` block does \
             not grant.\n\nDeclaring a tool describes its signature; granting it \
             authorises this agent to call it. Add the tool to the `tools` block."
        }
        UNRESOLVED_INTERPOLATION => {
            "A `${...}` placeholder in a prompt refers to a name that is not in \
             scope.\n\nPrompt interpolations are type-checked like any other \
             expression, which is what turns a silent empty substitution at \
             runtime into a compile error."
        }
        TYPE_MISMATCH => {
            "An expression produced a type the surrounding context does not \
             accept.\n\nIngot v0.1 performs no implicit conversions other than \
             int -> float. Convert explicitly or change the declared type."
        }
        ARGUMENT_COUNT_MISMATCH => {
            "A tool, verifier or agent was called with the wrong number of \
             arguments.\n\nThe declaration is the single source of truth for the \
             signature; update the call or the declaration."
        }
        PARALLEL_BODY_MUST_YIELD_VALUE => {
            "The body of `parallel map` must end in an expression statement.\n\n\
             `parallel map xs as x { ... }` evaluates to a list built from the \
             value of the last statement in each iteration, so that value must \
             exist and have a type."
        }
        EMIT_TYPE_MISMATCH => {
            "`emit` was given a value whose type differs from the agent's \
             declared output content type.\n\nFor `-> report<markdown>` the \
             emitted value must be `markdown`."
        }
        DENIED_CAPABILITY => {
            "A tool call requires an effect that the agent's policy denies.\n\n\
             Ingot is default-deny: an effect is available only if the policy \
             block explicitly allows it. Either grant the capability in the \
             `policy` block or call a tool that does not need it."
        }
        MISSING_POLICY_RULE => {
            "A tool call requires an effect for which the agent declares no \
             policy rule at all.\n\nBecause Ingot is default-deny, an absent rule \
             is a denial. Add an explicit rule so the intent is visible in the \
             source and in the compiled artifact."
        }
        APPROVAL_INSERTED => {
            "The policy marks this effect as `require approval`, so the compiler \
             inserted an explicit `approval` node before the call in the IR.\n\n\
             This is informational: the approval checkpoint is now part of the \
             compiled program and every backend must honour it."
        }
        UNBOUNDED_LOOP => {
            "A `loop` has no `max` bound.\n\nEvery loop in Ingot must have a \
             statically known upper bound so that step and cost budgets can be \
             checked before the agent runs. Write `loop max 10 { ... }`."
        }
        OUTPUT_NEVER_EMITTED => {
            "The agent declares an output but no reachable `emit` assigns it.\n\n\
             An agent that cannot produce its declared output would fail only at \
             runtime, so this is rejected at compile time."
        }
        UNUSED_BINDING => {
            "A binding is introduced but never read.\n\nThis is usually a typo in \
             a later reference. Prefix the name with `_` to keep it deliberately."
        }
        OUTPUT_NOT_ON_ALL_PATHS => {
            "Some execution paths reach the end of the flow without emitting the \
             declared output.\n\nAdd an `else` branch that emits, or move the \
             `emit` after the branch. Loops never count as guaranteed, because a \
             bounded loop may run zero times."
        }
        INVALID_IN_PARALLEL => {
            "`emit`, `checkpoint` and writes to `state` are not allowed inside a \
             `parallel map` body.\n\nIterations run concurrently and in an \
             unspecified order, so these would make the result depend on \
             scheduling. Collect values from the map and act on them afterwards."
        }
        RECURSIVE_AGENT => {
            "An agent calls itself, directly or through another agent.\n\n\
             Recursion has no static bound, so step and cost budgets could not be \
             checked. Model repetition with `loop max N` instead."
        }
        FUNCTION_NOT_PURE => {
            "A `fn` helper body contains agent work or another construct that \
             cannot be erased into a pure value.\n\nHelpers are source-level \
             conveniences: they inline into Agent IR instead of adding runtime \
             function calls. Keep the body to parameters, literals, lists, \
             field reads, pure builtins and operators."
        }
        UNSUPPORTED_TRANSPORT => {
            "The `tools` block names a transport this language version does not \
             support.\n\nLanguage 0.1 grants tools over MCP only: write \
             `mcp web.search`."
        }
        STATIC_STEPS_EXCEED_BUDGET => {
            "The flow performs more model and tool calls than the `steps` budget \
             allows, even on its shortest path.\n\nThe agent could never complete, \
             so this is rejected before it runs. Raise the budget or shorten the \
             flow."
        }
        _ => return None,
    };
    Some(text)
}

/// Every code that has a long-form explanation, for docs generation and tests.
pub const EXPLAINED_CODES: &[&str] = &[
    UNEXPECTED_CHARACTER,
    UNTERMINATED_STRING,
    UNSUPPORTED_LANGUAGE_VERSION,
    MISSING_LANGUAGE_DECLARATION,
    UNRESOLVED_NAME,
    TOOL_NOT_GRANTED,
    UNRESOLVED_INTERPOLATION,
    TYPE_MISMATCH,
    ARGUMENT_COUNT_MISMATCH,
    PARALLEL_BODY_MUST_YIELD_VALUE,
    EMIT_TYPE_MISMATCH,
    DENIED_CAPABILITY,
    MISSING_POLICY_RULE,
    APPROVAL_INSERTED,
    UNBOUNDED_LOOP,
    OUTPUT_NEVER_EMITTED,
    UNUSED_BINDING,
    OUTPUT_NOT_ON_ALL_PATHS,
    INVALID_IN_PARALLEL,
    RECURSIVE_AGENT,
    FUNCTION_NOT_PURE,
    UNSUPPORTED_TRANSPORT,
    STATIC_STEPS_EXCEED_BUDGET,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_code_has_an_explanation() {
        for code in EXPLAINED_CODES {
            assert!(explain(code).is_some(), "missing explanation for {code}");
        }
    }

    #[test]
    fn explain_is_case_insensitive() {
        assert_eq!(explain("ing4001"), explain("ING4001"));
    }

    #[test]
    fn unknown_code_has_no_explanation() {
        assert!(explain("ING9999").is_none());
    }
}
