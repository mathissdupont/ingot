// --- saying it in words -----------------------------------------------------
//
// Everything here translates an identifier into the sentence a person was
// looking for. It is a **presentation layer, and deliberately not a rename**:
// the replies keep `model_access` and `artifact<markdown>` because other
// programs read them and two of those shapes have a written schema. Renaming the
// data would be a contract break wearing a copy edit.
//
// So the rule for anything added below: translate at the last moment, on the
// way to the screen, and never on the way in.

const EFFECT_WORDS = {
  model_access: "calls a model",
  human: "asks a person",
  network: "reaches the network",
  filesystem_read: "reads files",
  filesystem_write: "writes files",
  external_write: "writes outside this machine",
  secrets: "reads a secret",
};

// An unknown effect is shown as it is rather than guessed at: a new capability
// should look unfamiliar, not be described wrongly.
function effectWords(effect) {
  return EFFECT_WORDS[effect] || effect;
}

// The IR spells an output `artifact<markdown>`; the source says
// `report<markdown>`, and what a person means is `markdown`.
function typeWords(type) {
  const wrapped = /^artifact<(.+)>$/.exec(String(type || ""));
  return wrapped ? wrapped[1] : type;
}

// A qualified name earns its keep when two projects declare the same short one,
// so the package is kept — underneath, not as the heading.
function splitName(name) {
  const parts = String(name || "").split(".");
  const short = parts.pop();
  return { short: short || String(name || ""), scope: parts.join(".") };
}

function tokenWords(count) {
  if (!count) return null;
  if (count >= 1000) return Math.round(count / 1024) + "k context";
  return count + " tokens of context";
}

// What the agent needs from a model.
//
// `model` carries two different things depending on the requirement: a pinned
// reference for an exact one, and otherwise the *kind* of requirement it is. So
// `capabilities` used to appear on screen as the agent's model — which is the
// name of a variant, and told nobody anything.
function modelWords(agent) {
  const kind = agent.model;
  if (kind === "unspecified") return "any model";
  if (kind !== "capabilities") return kind;

  const wants = (agent.modelRequires || []).map((name) => name.replace(/_/g, " "));
  const context = tokenWords(agent.modelContextMin);
  if (context) wants.push(context);
  return wants.length ? "needs " + wants.join(", ") : "any model";
}

const ORDINALS = ["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth"];

/// "the first question", counting from a zero-based index.
function ordinalWords(index) {
  return ORDINALS[index] || "number " + (index + 1);
}
