// --- the track, and the next job -------------------------------------------
//
// Both are `ingot doctor`, read twice: once grouped into four rungs, and once
// to find the first thing that is not passing. Nothing here invents a step, a
// score or a percentage — every rung is a subject the doctor already reports
// on, so the page and the command line cannot disagree about how far along a
// project is.
//
// Two rungs a person might expect are deliberately absent, because no check
// answers them yet: whether the project has a recorded cassette, and whether it
// has been packaged. They belong here when the CLI can say so.

// Each rung names the one check that answers its question. That check decides
// the rung; the rest of the subject's checks are notes beside it.
//
// This is not a shortcut, it is the difference between two questions. Reporting
// the worst status in a subject answers "is anything here imperfect", and a
// project with a working local model showed **amber** on Model because
// `provider.anthropic`, `provider.google` and `provider.openai` each warn that
// their API key is unset — three providers it does not use. The run worked. The
// rung was wrong. What a person is asking is "can this project make a model
// call", and `provider.default` is the check that answers it.
const STEPS = [
  {
    key: "source",
    decides: "source.compile",
    label: "Compiles",
    opens: "Unlocks everything after it",
  },
  {
    key: "provider",
    decides: "provider.default",
    label: "Model",
    opens: "Unlocks running against a live model",
  },
  {
    key: "tools",
    decides: "tools.routes",
    label: "Tools",
    opens: "Unlocks the tools this agent calls",
  },
  {
    key: "container",
    decides: "container.runtime",
    label: "Boundary",
    opens: "Unlocks running behind the boundary",
  },
];

// A subject with no checks is `idle` rather than passing: nothing was reported,
// which is not the same as nothing being wrong.
function stepStatus(checks, step) {
  const mine = checks.filter((check) => (check.id || "").indexOf(step.key + ".") === 0);
  if (!mine.length) return "idle";

  // A failure is never merely informational, wherever in the subject it is, so
  // it pulls the rung down even when the deciding check passes.
  if (mine.some((check) => check.status === "fail")) return "fail";

  const decisive = mine.find((check) => check.id === step.decides);
  // No deciding check in this reply — an older or newer CLI. Fall back to the
  // worst status rather than reporting green off a check that is not there.
  if (!decisive) {
    return mine.some((check) => check.status === "warn") ? "warn" : "pass";
  }
  return decisive.status;
}

function tick() {
  return svgEl("svg", { viewBox: "0 0 13 13", width: 13, height: 13, "aria-hidden": "true" }, [
    svgEl("path", {
      d: "M2 6.8l3.2 3.2L11 3",
      fill: "none",
      stroke: "currentColor",
      "stroke-width": 2.2,
      "stroke-linecap": "round",
    }),
  ]);
}

function trackOf(readiness) {
  const checks = readiness.checks || [];
  const statuses = STEPS.map((step) => stepStatus(checks, step));
  const first = statuses.findIndex((status) => status !== "pass");

  const parts = [];
  STEPS.forEach((step, index) => {
    // The status is always worn, and `now` is added on top of it. A failing
    // step that happens not to be the current one must not look like a step
    // nobody has reached yet — which is what it looked like when the class was
    // one or the other.
    const status = statuses[index];
    let klass = "step " + status;
    if (index === first) klass += " now";

    parts.push(el("div", { class: klass }, [
      el("span", { class: "bead" }, [status === "pass" ? tick() : el("span", { text: String(index + 1) })]),
      el("span", { class: "label", text: step.label }),
    ]));
    if (index < STEPS.length - 1) {
      parts.push(el("span", { class: "rope" + (status === "pass" ? " pass" : "") }));
    }
  });

  return el("div", { class: "track" }, parts);
}

/// The first thing standing in the way, with the doctor's own instruction.
function nextJob(readiness) {
  const checks = readiness.checks || [];
  const deciding = STEPS.map((step) => step.decides);
  // A failure first, and after that only a warning on a check that decides a
  // rung. The same reason the rungs work this way: "`ANTHROPIC_API_KEY` is not
  // set" is a note for a project using a local model, and offering it as the
  // next thing to do sends somebody to get a key they do not need.
  const blocking = checks.find((check) => check.status === "fail")
    || checks.find((check) => check.status === "warn" && deciding.indexOf(check.id) >= 0);

  if (!blocking) {
    return el("div", { class: "next settled" }, [
      el("div", { class: "grow" }, [
        el("div", { class: "eyebrow", text: "Nothing is in the way" }),
        el("h3", { text: "This project is ready to run" }),
        el("p", { text: "Every check the doctor makes passes. Start a run from the Runs tab." }),
      ]),
    ]);
  }

  const subject = STEPS.find((step) => (blocking.id || "").indexOf(step.key + ".") === 0);
  return el("div", { class: "next" }, [
    el("div", { class: "grow" }, [
      el("div", { class: "eyebrow", text: "Do this next" }),
      el("h3", { text: blocking.summary }),
      blocking.fix ? el("p", { text: blocking.fix }) : null,
      el("div", { class: "where", text: blocking.location }),
      subject ? el("div", { class: "unlock", text: subject.opens }) : null,
    ]),
  ]);
}
