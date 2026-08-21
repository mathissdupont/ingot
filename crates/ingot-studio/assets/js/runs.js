function renderLaunches(inner) {
  const launches = (state.launches || []).slice().reverse();
  if (!launches.length) return;

  const items = launches.map((launch) => el("div", { class: "row" }, [
    el("span", { class: "stripe " + LAUNCH_CHIP[launch.state] }),
    el("div", { class: "grow" }, [
      el("div", {}, [
        el("b", {
          text: launchAgent(launch)
            ? splitName(launchAgent(launch)).short
            : "the agent it declares",
        }),
        el("span", { class: "sub", text: "  process " + launch.pid + "  ·  " + launch.provider + "  ·  " + when(launch.startedUnix) }),
      ]),
      launch.state === "failed"
        ? el("div", { class: "fix", text: "exit " + launch.exitCode + " — the log below is what it said" })
        : null,
      // The panel that answers a question lives on the conversation tab, where
      // the question has the rest of the exchange around it. What belongs here
      // is that the process is stopped, and the way to it.
      launch.pending
        ? el("div", { class: "add", style: "margin-top:6px" }, [
            el("button", {
              class: "action",
              text: launch.pending.waitingFor === "question" ? "Answer its question" : "Decide on this",
              onclick: () => show("project", { tab: "conversation", chatId: null, runId: null, run: null }),
            }),
          ])
        : null,
      launch.log && launch.state !== "running" ? el("pre", { class: "block", style: "margin-top:8px", text: launch.log.trimEnd() }) : null,
      launch.output ? el("pre", { class: "block", style: "margin-top:8px", text: launch.output.trimEnd() }) : null,
      launch.truncated ? el("div", { class: "fix", text: "output was longer than the studio keeps" }) : null,
    ]),
    launch.state === "running"
      ? el("button", {
          class: "action quiet",
          text: "Stop",
          onclick: async () => {
            try {
              const answer = await api("launch?" + q({ path: state.path, pid: launch.pid }), { method: "DELETE" });
              state.runs = answer.runs;
              state.launches = answer.launches;
            } catch (error) { failed("That run was not stopped", error); }
            render();
          },
        })
      : null,
    el("span", { class: "chip " + LAUNCH_CHIP[launch.state], text: launch.state }),
  ]));

  inner.appendChild(card("Started from here", rows(items, ""),
    launches.some((launch) => launch.state !== "running")
      ? el("button", {
          class: "action quiet",
          text: "Clear finished",
          onclick: async () => {
            try {
              const answer = await api("launches?" + q({ path: state.path }), { method: "POST" });
              state.runs = answer.runs;
              state.launches = answer.launches;
            } catch (error) { failed("The finished runs were not cleared", error); }
            render();
          },
        })
      : null));
}

function renderRuns(inner) {
  if (state.runId) return renderRun(inner);
  if (!state.runs) return inner.appendChild(el("div", { class: "empty", text: "Reading run history…" }));

  inner.appendChild(startPanel());
  renderLaunches(inner);

  inner.appendChild(el("div", { class: "banner note", text: "A run writes its own event stream here as it goes. This is the only thing the studio stores about a project, it lives under the project's build directory, and deleting it loses only the history." }));

  const items = state.runs.map((run) => el("div", { class: "row click", onclick: () => show("project", { tab: "runs", runId: run.id, run: null }) }, [
    el("span", { class: "stripe " + STATE_CHIP[run.state] }),
    el("div", { class: "grow" }, [
      el("div", {}, [
        el("b", { text: splitName(run.agent).short }),
        el("span", { class: "sub", text: "  " + when(run.startedUnix) }),
      ]),
      el("div", { class: "sub", text: [run.provider, run.contained ? "contained" : null, lasted(run)].filter(Boolean).join("  ·  ") }),
      run.reason ? el("div", { class: "fix", text: run.reason }) : null,
    ]),
    el("div", { class: "sub", text: run.usage ? run.usage.inputTokens + " in / " + run.usage.outputTokens + " out" : "" }),
    run.cost ? el("div", { class: "sub", text: run.cost }) : null,
    el("span", { class: "chip " + STATE_CHIP[run.state], text: STATE_WORD[run.state] }),
  ]));

  inner.appendChild(card("Runs", rows(items, "No run has been recorded for this project yet. `ingot run` writes one each time.")));
}

function renderRun(inner) {
  if (!state.run) return inner.appendChild(el("div", { class: "empty", text: "Reading the run…" }));
  const run = state.run;

  inner.appendChild(el("div", { class: "add", style: "margin-bottom:16px" }, [
    el("button", { class: "action", text: "← All runs", onclick: () => show("project", { tab: "runs", runId: null, run: null }) }),
    // The same run, read as what was said rather than as what was emitted.
    el("button", {
      class: "action",
      text: "Read as a conversation",
      onclick: () => show("project", { tab: "conversation", chatId: run.id, runId: null, run: null }),
    }),
    el("button", {
      class: "action quiet",
      text: "Delete this record",
      onclick: async () => {
        try {
          await api("run?" + q({ path: state.path, id: run.id }), { method: "DELETE" });
          show("project", { tab: "runs", runId: null, run: null, runs: null });
        } catch (error) { failed("That record was not deleted", error); render(); }
      },
    }),
  ]));

  inner.appendChild(card("Run",
    el("div", { class: "body" }, [
      el("dl", { class: "facts" }, [
        // The facts list keeps the qualified name: it is the one place on the
        // page that answers "which agent exactly", and a package is half of that
        // answer.
        el("dt", { text: "agent" }), el("dd", { text: run.agent }),
        el("dt", { text: "provider" }), el("dd", { text: run.provider }),
        el("dt", { text: "started" }), el("dd", { text: when(run.startedUnix) }),
        el("dt", { text: "finished" }), el("dd", { text: run.finishedUnix ? when(run.finishedUnix) + "  (" + lasted(run) + ")" : "—" }),
        el("dt", { text: "steps" }), el("dd", { text: run.steps === null ? "—" : String(run.steps) }),
        el("dt", { text: "tokens" }), el("dd", { text: run.usage ? run.usage.inputTokens + " in / " + run.usage.outputTokens + " out" : "—" }),
        el("dt", { text: "cost" }), el("dd", { text: run.cost || "not priced" }),
        run.reason ? el("dt", { text: "reason" }) : null,
        run.reason ? el("dd", { text: run.reason }) : null,
      ]),
    ]),
    el("span", { class: "chip " + STATE_CHIP[run.state], text: STATE_WORD[run.state] })));

  const lines = run.events.map((event) => el("div", { class: "line" + (event.event === "runFailed" ? " bad" : "") }, [
    el("span", { class: "kind", text: event.event }),
    el("span", { class: "what", text: describe(event) }),
  ]));
  inner.appendChild(card("Event stream",
    el("div", { class: "body flush trace" }, lines.length ? lines : [el("div", { class: "empty", text: "No event was recorded." })]),
    el("span", { class: "sub", text: run.events.length + " event(s)" })));
}

// The events are shown as they were recorded. This only decides which of an
// event's own fields to put on the line; it never computes a new fact.
function describe(event) {
  const parts = [];
  for (const key of Object.keys(event)) {
    if (key === "event") continue;
    const value = event[key];
    if (value === null || value === undefined) continue;
    parts.push(key + "=" + (typeof value === "object" ? JSON.stringify(value) : String(value)));
  }
  return parts.join("  ");
}
