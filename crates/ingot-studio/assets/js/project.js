// --- one project -----------------------------------------------------------

function renderProject(inner) {
  const project = state.projects.find((candidate) => candidate.path === state.path);
  const named = project ? (project.name || state.path) : state.path;
  inner.appendChild(head(named, project && project.description, state.path, named));

  // Read left to right it is a project's own order: what this is, what it does,
  // what it said, what it did, and what it is allowed to touch.
  const TAB_NAMES = {
    overview: "Overview",
    canvas: "Canvas",
    conversation: "Conversation",
    runs: "Runs",
    boundary: "Boundary",
  };
  // The mark rides on the tab rather than on the panel behind it, so a run that
  // needs somebody is visible from the other four.
  const mark = conversationMark();
  const tabs = el("div", { class: "tabs" },
    ["overview", "canvas", "conversation", "runs", "boundary"].map((name) =>
      el("button", {
        class: "tab",
        "aria-current": String(state.tab === name),
        title: name === "conversation" && mark ? MARK_WORDS[mark] : null,
        // Clicking the tab clears a pinned run: somebody arriving here wants the
        // conversation that is happening, not the one they last opened.
        onclick: () => show("project", { tab: name, runId: null, run: null, proposed: null, chatId: null }),
      }, [
        el("span", { text: TAB_NAMES[name] }),
        name === "conversation" && mark
          ? el("span", { class: "dot " + mark, role: "img", "aria-label": MARK_WORDS[mark] })
          : null,
      ])));
  inner.appendChild(tabs);

  if (state.tab === "runs") return renderRuns(inner);
  if (state.tab === "conversation") return renderConversation(inner);
  if (state.tab === "canvas") return renderCanvas(inner);
  if (!state.detail) return inner.appendChild(el("div", { class: "empty", text: "Reading the project…" }));
  if (state.tab === "boundary") return renderBoundary(inner, state.detail);
  renderOverview(inner, state.detail);
}

function renderOverview(inner, detail) {
  const readiness = detail.readiness;
  const failed = readiness.checks.filter((check) => check.status === "fail").length;
  const warned = readiness.checks.filter((check) => check.status === "warn").length;

  // Where this project stands, and the one thing to do about it, before any of
  // the detail. Both are the readiness report — the same one listed below —
  // read as a position rather than as a list.
  inner.appendChild(trackOf(readiness));
  inner.appendChild(nextJob(readiness));

  // An agent is a thing somebody made, so it is shown as one: its own face, and
  // the limits it declares as plain facts rather than as a score.
  inner.appendChild(card("Agents",
    rows(detail.agents.map((agent) => el("div", { class: "row agent" }, [
      sprite(agent.name, "card", "idle"),
      el("div", { class: "grow" }, [
        // The short name is the heading and the package is underneath, because
        // `demo.framing.FramingReport` is not what anybody called it.
        el("div", {}, [
          el("b", { text: splitName(agent.name).short }),
          el("span", { class: "sub", text: "  " + agent.steps + " step(s)" }),
        ]),
        splitName(agent.name).scope
          ? el("div", { class: "where", text: splitName(agent.name).scope })
          : null,
        el("div", { class: "sub", text: signature(agent) }),
        el("div", { class: "tags" }, [
          detail.compiles ? el("span", { class: "tag pass", text: "compiles" }) : null,
          el("span", { class: "tag" }, ["at most ", el("b", { text: String(agent.steps) }), " steps"]),
          agent.tools.length
            ? el("span", { class: "tag" }, [el("b", { text: String(agent.tools.length) }), " tool(s)"])
            : null,
          reachOf(agent).length
            ? el("span", { class: "tag" }, ["reaches ", el("b", { text: reachOf(agent).join(", ") })])
            : null,
          agent.effects.indexOf("human") >= 0
            ? el("span", { class: "tag asks", text: "asks you things" })
            : null,
        ]),
        agent.effects.length
          ? el("div", { class: "sub", text: "it " + agent.effects.map(effectWords).join(", ") })
          : null,
        agent.tools.length ? el("ul", { class: "bullets" }, agent.tools.map((tool) =>
          el("li", { text: tool.name + (tool.reach.length ? "  →  " + tool.reach.join(", ") : "") }))) : null,
      ]),
      el("span", { class: "chip idle", text: modelWords(agent) }),
    ])), "This program declares no agents.")));

  inner.appendChild(card("Readiness",
    rows(readinessRows(readiness.checks), "Nothing to check."),
    el("span", { class: "chip " + (readiness.ready ? "pass" : "fail"), text: readiness.ready ? "ready" : failed + " failing" })));

  inner.appendChild(card("Diagnostics",
    rows(detail.diagnostics.map((diagnostic) => el("div", { class: "row" }, [
      el("span", { class: "stripe " + (diagnostic.severity === "error" ? "fail" : "warn") }),
      el("div", { class: "grow" }, [
        el("div", {}, [el("code", { text: diagnostic.code || "—" }), " ", diagnostic.message]),
        el("div", { class: "where", text: whereWords(diagnostic.location, state.path) }),
      ]),
      el("span", { class: "chip " + (diagnostic.severity === "error" ? "fail" : "warn"), text: diagnostic.severity }),
    ])), detail.compiles ? "The source compiles with nothing to report." : "—"),
    el("span", { class: "sub", text: warned ? warned + " warning(s) in readiness" : "" })));
}

// The readiness list, grouped the way the track above it is grouped: by the
// subject in each check's id.
//
// It was one flat list, and on a healthy project that is twelve near-identical
// green rows with the two that need reading somewhere in the middle. So a
// subject where everything passes now collapses to a single line. Two rules keep
// that from being a way of losing things: a subject holding anything that is not
// a pass opens by itself, and a check whose id belongs to no subject this page
// knows gets a group of its own rather than being dropped — which is the failure
// mode a hard-coded list of four would have had the first time the CLI adds a
// fifth.
function readinessRows(checks) {
  const groups = STEPS.map((step) => ({ key: step.key, label: step.label, checks: [] }));
  const strays = { key: "other", label: "Other checks", checks: [] };
  for (const check of checks || []) {
    const id = check.id || "";
    const home = groups.find((group) => id.indexOf(group.key + ".") === 0);
    (home || strays).checks.push(check);
  }
  if (strays.checks.length) groups.push(strays);

  const out = [];
  for (const group of groups) {
    if (!group.checks.length) continue;
    const status = worstOf(group.checks);
    const decided = state.groups[group.key];
    const open = decided === undefined ? status !== "pass" : decided;

    out.push(el("button", {
      class: "row click group",
      "aria-expanded": String(open),
      onclick: () => { state.groups[group.key] = !open; render(); },
    }, [
      el("span", { class: "stripe " + status }),
      chevron(open),
      el("div", { class: "grow" }, [
        el("div", {}, [
          el("b", { text: group.label }),
          el("span", { class: "sub", text: "  " + countWords(group.checks) }),
        ]),
      ]),
      el("span", { class: "chip " + status, text: status }),
    ]));

    if (!open) continue;
    for (const check of group.checks) {
      out.push(el("div", { class: "row nested" }, [
        el("span", { class: "stripe " + check.status }),
        el("div", { class: "grow" }, [
          el("div", { text: check.summary }),
          el("div", { class: "where", text: whereWords(check.location, state.path) }),
          check.fix ? el("div", { class: "fix", text: "fix: " + check.fix }) : null,
        ]),
        el("span", { class: "chip " + check.status, text: check.status }),
      ]));
    }
  }
  return out;
}

// The worst thing in a group, which is what its one line has to report. Not the
// same question the track asks — a rung reports the check that *decides* it, and
// this reports whether reading further is worth it.
function worstOf(checks) {
  if (checks.some((check) => check.status === "fail")) return "fail";
  if (checks.some((check) => check.status === "warn")) return "warn";
  return checks.length ? "pass" : "idle";
}

function countWords(checks) {
  const failed = checks.filter((check) => check.status === "fail").length;
  const warned = checks.filter((check) => check.status === "warn").length;
  const total = checks.length;
  const said = [];
  if (failed) said.push(failed + " failing");
  if (warned) said.push(warned + " to look at");
  if (!said.length) return total === 1 ? "one check, passing" : total + " checks, all passing";
  return said.join(" and ") + ", of " + total;
}

// Turned by CSS rather than swapped for a second glyph, so the two states are
// one shape and the rotation says which way it goes.
function chevron(open) {
  return svgEl("svg", {
    class: "chev" + (open ? " open" : ""),
    viewBox: "0 0 12 12", width: 12, height: 12, "aria-hidden": "true",
  }, [
    svgEl("path", {
      d: "M4.2 2.4L7.8 6l-3.6 3.6",
      fill: "none", stroke: "currentColor", "stroke-width": 1.8,
      "stroke-linecap": "round", "stroke-linejoin": "round",
    }),
  ]);
}

// Every host this agent's tools may reach, once. The policy states it per tool;
// what a person wants to know is the union.
function reachOf(agent) {
  const seen = [];
  for (const tool of agent.tools || []) {
    for (const host of tool.reach || []) {
      if (seen.indexOf(host) < 0) seen.push(host);
    }
  }
  return seen;
}

function signature(agent) {
  const side = (list) => list.map((entry) => entry.name + ": " + typeWords(entry.type)).join(", ") || "—";
  return side(agent.inputs) + "  →  " + side(agent.outputs);
}

function renderBoundary(inner, detail) {
  const boundary = detail.boundary;
  if (boundary.problems.length) {
    inner.appendChild(el("div", { class: "banner", text: boundary.problems.join("\n") }));
  }
  // The agent's own boundary first, and always — not only when there is no plan
  // to show. Somebody reading a tool-server plan is the person most likely to
  // assume it covers the agent too.
  inner.appendChild(containedGuidance(detail));

  if (!boundary.plans.length) {
    inner.appendChild(card("Tool server boundaries", el("div", { class: "body" }, [
      el("p", { class: "muted", text: "A run with --sandbox puts each declared tool server in a box built from the policy of the agent that calls it. This project declares no tool server, so there is nothing here to plan." }),
    ])));
    return;
  }
  for (const plan of boundary.plans) {
    inner.appendChild(card(plan.server + "  ·  " + plan.agent,
      el("div", { class: "body flush" }, [
        el("pre", { class: "block", text: plan.rendered }),
      ]),
      el("span", { class: "chip " + (plan.enforced ? "pass" : "warn"), text: plan.enforced ? "fully enforced" : "partly unenforced" })));
  }
}
