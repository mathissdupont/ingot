// --- the boundary, as something you can switch on ---------------------------
//
// There are two boundaries and they are not degrees of one another. `--sandbox`
// puts each declared tool server in a box built from the agent's policy;
// `--contained` puts the agent itself in one, and applies whether or not any
// tool server exists. Either can be asked for without the other.
//
// This page used to describe both and offer neither, which is the worst of the
// three options: somebody reads that a boundary exists, cannot reach it, and
// concludes it is aspirational. So the switches are here — and, when this
// machine cannot raise a boundary, the reason and the command that fixes it are
// here too, rather than a switch that fails on being clicked.
//
// Nothing below decides whether a boundary is possible. Every fact comes from
// the readiness report, which is `ingot doctor` — so the page and the command
// line cannot disagree about it.

const CONTAINER_CHECKS = [
  "container.runtime",
  "container.reference-image",
  "container.configured-image",
  "container.image-version",
];

function containerChecks(detail) {
  const checks = (detail && detail.readiness && detail.readiness.checks) || [];
  return checks.filter((check) => CONTAINER_CHECKS.indexOf(check.id) >= 0);
}

// Why a contained run is not available, in a sentence, per check.
//
// The doctor's own summary is kept underneath rather than thrown away — for
// `container.runtime` it is a Docker socket error with a pipe name and a URL in
// it, which is the right thing to show somebody debugging and the wrong thing to
// answer "why is this switch off" with.
const WHY_NOT = {
  "container.runtime":
    "Nothing on this machine is running containers, so there is no box to put the agent in.",
  "container.reference-image":
    "The image a contained run needs is not on this machine. Ingot never downloads it — it is built here, from this binary.",
  "container.configured-image":
    "The image this project asks for is not on this machine, and Ingot will not fetch it for you.",
  "container.image-version":
    "The image this project asks for was built by a different version of Ingot.",
};

// Whether the agent itself can be put in a box right now, and if not, the first
// thing standing in the way.
//
// A warning counts as standing in the way, and that is deliberate: on this
// subject the warnings are all of the form "cannot be inspected without a
// runtime", which is not a caveat about a contained run — it is the reason there
// will not be one.
function containedReadiness(detail) {
  const checks = containerChecks(detail);
  if (!checks.length) {
    return { ready: false, why: "This project's readiness report says nothing about containment." };
  }
  const stopper = checks.find((check) => check.status === "fail") ||
    checks.find((check) => check.status === "warn");
  if (!stopper) return { ready: true };
  return {
    ready: false,
    why: WHY_NOT[stopper.id] || stopper.summary,
    detail: WHY_NOT[stopper.id] ? stopper.summary : null,
    fix: stopper.fix,
  };
}

// Whether each declared tool server can be put in a box.
//
// A plan that is only partly enforced is refused rather than offered with a
// warning. The command line has a flag for accepting one — it is a deliberate
// acceptance of a weaker boundary, and a checkbox on a web page is the wrong
// place to make that decision quickly.
function sandboxReadiness(detail) {
  const plans = (detail && detail.boundary && detail.boundary.plans) || [];
  if (!plans.length) return { none: true };
  const weak = plans.filter((plan) => !plan.enforced);
  if (weak.length) {
    return {
      ready: false,
      why: weak.length + " of " + plans.length + " tool server boundaries would be only partly enforced.",
      fix: "tighten the policy, or accept it explicitly in a terminal with `ingot run --sandbox --sandbox-allow-unenforced`",
    };
  }
  return { ready: true };
}

// A fix, with the command in it made ready to run.
//
// The doctor writes its fixes for a person at a prompt, so they carry a command
// in backticks. Pulling it out and offering to copy it is the whole distance
// between being told what to do and having done it.
function fixLine(fix) {
  const command = /`(ingot [^`]+)`/.exec(String(fix || ""));
  return el("div", { class: "guide" }, [
    el("div", { class: "fix", text: fix }),
    command ? commandBlock(command[1]) : null,
  ]);
}

function commandBlock(command) {
  const button = el("button", { class: "action quiet", text: "Copy" });
  button.addEventListener("click", () => {
    // No fallback when the clipboard is unavailable, because there is a good
    // one already: the command is on the screen, selectable, next to the
    // button. A silent failure is worse than a button that says what happened.
    const copied = navigator.clipboard && navigator.clipboard.writeText(command);
    if (!copied) return void (button.textContent = "select it and copy");
    copied.then(
      () => { button.textContent = "copied"; },
      () => { button.textContent = "select it and copy"; },
    );
  });
  return el("div", { class: "command" }, [el("code", { text: command }), button]);
}

// The switch itself. One row, and it says the same thing whether it is on or
// off: what a contained run is, and what it costs.
function boundarySwitch(label, note, readiness, onchange) {
  const box = el("input", { type: "checkbox", disabled: !readiness.ready, onchange: (event) => onchange(event.target.checked) });
  return el("div", { class: "switch" + (readiness.ready ? "" : " blocked") }, [
    el("label", {}, [box, el("span", { text: label })]),
    el("div", { class: "sub", text: readiness.ready ? note : readiness.why }),
    readiness.ready || !readiness.detail ? null : el("div", { class: "where", text: readiness.detail }),
    readiness.ready || !readiness.fix ? null : fixLine(readiness.fix),
  ]);
}

// --- the boundary tab's own guidance ---------------------------------------
//
// The tab used to say what the two boundaries are only when there was no plan
// to show, which is precisely backwards: somebody looking at a plan is the
// person most likely to think it covers the agent as well.
function containedGuidance(detail) {
  const readiness = containedReadiness(detail);
  const checks = containerChecks(detail);

  return card("Running the agent behind the boundary",
    el("div", { class: "body" }, [
      el("p", { class: "muted", text: "A run with --contained puts the agent itself in a container built from its own policy. The model call and any question it asks you cross that boundary; nothing else does. It applies whether or not this project declares a tool server — which is what makes it different from the plans below." }),
      readiness.ready
        ? el("p", { class: "ok-note", text: "This machine can do it. The switch is on the Runs tab, beside the start button." })
        : el("p", { class: "muted", text: "This machine cannot do it yet, and it takes three things: a container runtime that is running, Linux containers rather than Windows ones, and the image that matches this binary — Ingot never pulls that image, it is built locally." }),
      checks.length
        ? el("div", { class: "checks" }, checks.map((check) => el("div", { class: "line" }, [
            el("span", { class: "stripe " + check.status }),
            el("div", { class: "grow" }, [
              el("div", { text: check.summary }),
              check.fix ? fixLine(check.fix) : null,
            ]),
          ])))
        : null,
    ]),
    el("span", { class: "chip " + (readiness.ready ? "pass" : "warn"), text: readiness.ready ? "available" : "not available here" }));
}
