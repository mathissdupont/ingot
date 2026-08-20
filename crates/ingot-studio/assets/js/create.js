// --- starting from nothing --------------------------------------------------
//
// The one thing this page did that nothing else does: write a project that was
// not there. Everything else it shows is a directory read twice.
//
// It exists because of the first minute. Somebody installs Ingot, opens the
// studio, and has no project — so the very first thing the page could tell them
// was to go and use a terminal, which is the sentence the studio was built to
// remove.
//
// What it writes is exactly what `ingot new --template …` writes, because the
// server calls that same function. A studio with its own idea of a starter would
// be a second answer to "what does a new project look like", and the two would
// drift.
//
// **No model is involved.** `ingot new` can also author from a description with a
// provider; that spends money and needs a key, and neither belongs behind a
// button labelled Create. The description here only picks the template and
// becomes the project's own description.

const TEMPLATES = [
  {
    id: "",
    name: "Let the description choose",
    what: "Picks between the two below from the words you wrote.",
  },
  {
    id: "brief",
    name: "A brief",
    what: "One typed input, one model call, one markdown artifact. The smallest thing that is still a real agent.",
  },
  {
    id: "document-workflow",
    name: "A document workflow",
    what: "Two inputs and a checked-in document, transformed for an audience.",
  },
];

// Kept as one live node for the same reason the start panel is: a rebuild takes
// the caret out of a half-typed path.
function createPanel() {
  if (state.createForm) return state.createForm;

  const chosen = { directory: "", template: "", workflow: "" };

  const directory = el("input", {
    class: "text",
    placeholder: "the whole path of the directory to create",
    spellcheck: "false",
    oninput: (event) => { chosen.directory = event.target.value; },
  });
  const workflow = el("input", {
    class: "text",
    placeholder: "what should it do? e.g. summarise an incident report for an executive",
    spellcheck: "false",
    oninput: (event) => { chosen.workflow = event.target.value; drawWhat(); },
  });
  const picker = el("select", { class: "text", onchange: (event) => { chosen.template = event.target.value; drawWhat(); } },
    TEMPLATES.map((template) => el("option", { value: template.id, text: template.name })));

  const what = el("div", { class: "sub" });
  function drawWhat() {
    const template = TEMPLATES.find((candidate) => candidate.id === chosen.template) || TEMPLATES[0];
    what.textContent = template.what;
  }

  const button = el("button", { class: "action", text: "Create it" });
  button.addEventListener("click", async () => {
    if (!chosen.directory.trim()) return;
    button.disabled = true;
    try {
      const body = { directory: chosen.directory, workflow: chosen.workflow };
      if (chosen.template) body.template = chosen.template;
      const answer = await api("create", {
        method: "POST",
        headers: { "X-Ingot-Token": TOKEN, "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      state.projects = answer.projects;
      state.error = null;
      // Created, bookmarked, and opened — the three things somebody wanted when
      // they typed a path, rather than the first of them.
      const made = answer.projects.find((project) => project.path === chosen.directory.trim()) ||
        answer.projects[answer.projects.length - 1];
      button.disabled = false;
      state.createForm = null;
      if (made) return show("project", { path: made.path, tab: "overview", detail: null, runs: null, run: null, runId: null, chatId: null });
    } catch (error) {
      failed("Nothing was created", error);
    }
    button.disabled = false;
    render();
  });

  const node = card("Create a project", el("div", { class: "body" }, [
    el("label", { class: "field" }, [el("span", { class: "sub", text: "where" }), directory]),
    el("label", { class: "field" }, [el("span", { class: "sub", text: "what for" }), workflow]),
    el("label", { class: "field" }, [el("span", { class: "sub", text: "start from" }), picker]),
    what,
    el("div", { class: "add", style: "margin-top:14px" }, [
      button,
      el("span", { class: "sub", text: "It writes a manifest, a main.ing that compiles, a cassette to replay and a README. Nothing is asked of a model, and nothing existing is overwritten." }),
    ]),
  ]));

  drawWhat();
  state.createForm = node;
  return node;
}
