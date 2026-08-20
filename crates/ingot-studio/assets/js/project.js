// --- one project -----------------------------------------------------------

function renderProject(inner) {
  const project = state.projects.find((candidate) => candidate.path === state.path);
  inner.appendChild(head(project ? (project.name || state.path) : state.path, project && project.description, state.path));

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

  inner.appendChild(card("Readiness",
    rows(readiness.checks.map((check) => el("div", { class: "row" }, [
      el("span", { class: "stripe " + check.status }),
      el("div", { class: "grow" }, [
        el("div", { text: check.summary }),
        el("div", { class: "where", text: check.location }),
        check.fix ? el("div", { class: "fix", text: "fix: " + check.fix }) : null,
      ]),
      el("span", { class: "chip " + check.status, text: check.status }),
    ])), "Nothing to check."),
    el("span", { class: "chip " + (readiness.ready ? "pass" : "fail"), text: readiness.ready ? "ready" : failed + " failing" })));

  inner.appendChild(card("Diagnostics",
    rows(detail.diagnostics.map((diagnostic) => el("div", { class: "row" }, [
      el("span", { class: "stripe " + (diagnostic.severity === "error" ? "fail" : "warn") }),
      el("div", { class: "grow" }, [
        el("div", {}, [el("code", { text: diagnostic.code || "—" }), " ", diagnostic.message]),
        el("div", { class: "where", text: diagnostic.location }),
      ]),
      el("span", { class: "chip " + (diagnostic.severity === "error" ? "fail" : "warn"), text: diagnostic.severity }),
    ])), detail.compiles ? "The source compiles with nothing to report." : "—"),
    el("span", { class: "sub", text: warned ? warned + " warning(s) in readiness" : "" })));

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
