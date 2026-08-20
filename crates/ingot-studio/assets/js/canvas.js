// --- the canvas -------------------------------------------------------------
//
// A two-way view of the file, and never a second source of truth: every gesture
// here produces a byte range and a replacement, and the file decides what it
// means. See RFC-0016.

const BLOCK_LABEL = {
  modelCall: "asks a model",
  question: "asks a person",
  toolCall: "calls a tool",
  memoryWrite: "writes working memory",
  check: "checks",
  marker: "checkpoint",
  output: "emits",
  container: "if / loop",
  fanOut: "fan-out",
  unreadable: "unreadable",
  unknown: "not drawn",
};

/// The lines an edit touches, before and after.
///
/// Shown for every gesture, because applying an edit without showing it is the
/// one thing this surface must not do. Cheap here and nowhere else: the canvas
/// already has the exact range and the exact replacement.
function previewOf(source, edit) {
  const lineStart = source.lastIndexOf("\n", edit.startByte - 1) + 1;
  let lineEnd = source.indexOf("\n", edit.endByte);
  if (lineEnd === -1) lineEnd = source.length;
  const before = source.slice(lineStart, lineEnd);
  const after = source.slice(lineStart, edit.startByte) + edit.newText + source.slice(edit.endByte, lineEnd);
  return { before, after };
}

function propose(edit) {
  state.proposed = edit;
  render();
}

async function applyProposed() {
  const edit = state.proposed;
  if (!edit) return;
  try {
    const answer = await api("canvas?" + q({ path: state.path }), {
      method: "POST",
      headers: { "X-Ingot-Token": TOKEN, "Content-Type": "application/json" },
      body: JSON.stringify({
        startByte: edit.startByte,
        endByte: edit.endByte,
        expected: edit.expected,
        newText: edit.newText,
      }),
    });
    state.canvas = answer;
    state.proposed = null;
    // The file changed, so everything read from it is stale.
    state.detail = null;
  } catch (error) {
    state.error = String(error.message || error);
  }
  render();
}

function renderProposed(inner, source) {
  if (!state.proposed) return;
  const { before, after } = previewOf(source, state.proposed);
  inner.appendChild(el("div", { class: "proposed" }, [
    el("b", { text: "This is what will be written" }),
    el("pre", {}, [
      el("span", { class: "del", text: "- " + before + "\n" }),
      el("span", { class: "ins", text: "+ " + after }),
    ]),
    el("div", { class: "answer", style: "display:flex;gap:8px;margin-top:10px" }, [
      el("button", { class: "action", text: "Write it", onclick: applyProposed }),
      el("button", { class: "action quiet", text: "Leave it", onclick: () => { state.proposed = null; render(); } }),
    ]),
  ]));
}

function leafField(source, leaf) {
  const multiline = leaf.kind === "text" && leaf.text.length > 48;
  const field = el(multiline ? "textarea" : "input", {
    value: leaf.text,
    rows: multiline ? 3 : null,
    spellcheck: "false",
    // Committed on blur. Per keystroke would recompile under the caret; an
    // explicit save button would also be forgotten.
    onblur: (event) => {
      const next = event.target.value;
      if (next === leaf.text) return;
      propose({
        startByte: leaf.span.startByte,
        endByte: leaf.span.endByte,
        expected: leaf.text,
        newText: next,
      });
    },
  });
  if (multiline) field.textContent = leaf.text;
  return el("label", { class: "leaf" }, [el("span", { text: leaf.role }), field]);
}

function renderBlock(source, block) {
  if (!block.editable) {
    return el("div", { class: "block readonly" }, [
      el("div", { class: "what", text: BLOCK_LABEL[block.kind] || block.kind }),
      // Shown as source, in place, so the order stays legible and it is obvious
      // what has to be opened in an editor.
      el("pre", { class: "block", style: "margin-top:6px", text: block.source }),
    ]);
  }
  return el("div", { class: "block" }, [
    el("div", { class: "what", text: BLOCK_LABEL[block.kind] || block.kind }),
    block.binding ? el("div", { class: "sub", text: "binds " + block.binding }) : null,
    ...block.leaves.map((leaf) => leafField(source, leaf)),
    block.children.length
      ? el("div", { class: "kids" }, block.children.map((child) => renderBlock(source, child)))
      : null,
  ]);
}

function renderCanvas(inner) {
  if (!state.canvas) return inner.appendChild(el("div", { class: "empty", text: "Drawing the flow…" }));
  const drawn = state.canvas.canvas;
  if (!drawn) return inner.appendChild(el("div", { class: "empty", text: "This project declares no agent with a flow to draw." }));
  const source = state.canvas.source || "";

  renderProposed(inner, source);

  const notes = state.canvas.diagnostics || [];
  if (notes.length) {
    inner.appendChild(card("What the compiler says", rows(notes.map((note) =>
      el("div", { class: "row" }, [
        el("span", { class: "chip " + (note.severity === "error" ? "fail" : note.severity === "warning" ? "warn" : "idle"), text: note.code }),
        el("div", { class: "grow" }, [
          el("div", { text: note.message }),
          el("div", { class: "where", text: note.location }),
        ]),
      ])), "")));
  }

  inner.appendChild(card(drawn.agent + " — flow",
    el("div", { style: "padding:12px 16px" }, drawn.blocks.map((block) => renderBlock(source, block)))));

  if (drawn.edges.length) {
    inner.appendChild(card("What reads what",
      el("div", { class: "edges", style: "padding:12px 16px" },
        drawn.edges.map((edge) => el("div", { text: edge.from + " → " + edge.to + "   " + edge.name })))));
  }

  if (drawn.policy.length) {
    inner.appendChild(card("Policy",
      el("div", { style: "padding:12px 16px" }, drawn.policy.map((rule) =>
        el("div", { class: "block" }, [
          el("div", { class: "what", text: rule.subject }),
          ...rule.leaves.filter((leaf) => leaf.role === "action").map((leaf) => leafField(source, leaf)),
        ])))));
  }
}
