// --- rendering -------------------------------------------------------------

function render() {
  renderRail();
  const inner = document.getElementById("inner");
  inner.textContent = "";
  if (state.error) inner.appendChild(el("div", { class: "banner", text: state.error }));
  if (state.view === "projects") renderProjects(inner);
  else if (state.view === "machine") renderMachine(inner);
  else if (state.view === "project") renderProject(inner);
}

function renderRail() {
  for (const link of document.querySelectorAll(".rail-link")) {
    const current = link.dataset.view === state.view || (state.view === "project" && link.dataset.view === "projects");
    link.setAttribute("aria-current", String(current));
  }
  const list = document.getElementById("project-list");
  list.textContent = "";
  if (!state.projects.length) {
    list.appendChild(el("div", { class: "empty", text: "No projects yet." }));
  }
  for (const project of state.projects) {
    list.appendChild(el("button", {
      class: "project-link",
      "aria-current": String(state.view === "project" && state.path === project.path),
      title: project.path,
      onclick: () => show("project", { path: project.path, tab: "overview", runId: null, detail: null, runs: null, run: null }),
    }, [
      // Its own face, from its own name, so the list is recognisable by shape
      // before it is read.
      sprite(project.name || project.path, "bust", "idle"),
      el("span", { class: "name", text: project.name || project.path }),
      project.problem ? el("span", { class: "stripe fail" }) : null,
    ]));
  }
}

// `face` is the name the header's sprite is drawn from, when the thing named
// has one. A machine does not, so the argument is optional rather than a sprite
// being invented for every page.
function head(title, subtitle, path, face) {
  return el("header", { class: "page-head" }, [
    face ? sprite(face, "card", "idle") : null,
    el("div", { class: "said" }, [
      el("h1", { text: title }),
      subtitle ? el("p", { text: subtitle }) : null,
      path ? el("p", { class: "path", title: path, text: whereWords(path) }) : null,
    ]),
  ]);
}

/// Nothing here yet: what this place is for, and the one thing that fills it.
///
/// Takes the action rather than assuming one. A tab with nothing to do is
/// allowed to say so and stop there, and an invented button would be worse than
/// the emptiness it was hiding.
function hollow(face, mood, title, lines, action) {
  return el("div", { class: "hollow" }, [
    face ? sprite(face, "tall", mood || "idle") : null,
    el("h3", { text: title }),
    ...[].concat(lines || []).map((line) => el("p", { text: line })),
    action ? el("div", { class: "add" }, [action]) : null,
  ]);
}

// `title` is a label, so the strip uppercases it. `named` is somebody's
// identifier and is exempt: `FramingReport` set as `FRAMINGREPORT` loses the
// case that made it readable, which is a caption destroying the thing it
// captions.
function card(title, body, aside, named) {
  return el("div", { class: "card" }, [
    el("h3", {}, [
      el("span", {}, [
        named ? el("span", { class: "named", text: named }) : null,
        el("span", { text: title }),
      ]),
      aside || null,
    ]),
    body,
  ]);
}

function rows(items, empty) {
  if (!items.length) return el("div", { class: "body flush" }, [el("div", { class: "empty", text: empty })]);
  return el("div", { class: "body flush" }, items);
}
