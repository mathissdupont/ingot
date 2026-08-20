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

function head(title, subtitle, path) {
  return el("header", { class: "page-head" }, [
    el("h1", { text: title }),
    subtitle ? el("p", { text: subtitle }) : null,
    path ? el("p", { class: "path", text: path }) : null,
  ]);
}

function card(title, body, aside) {
  return el("div", { class: "card" }, [
    el("h3", {}, [el("span", { text: title }), aside || null]),
    body,
  ]);
}

function rows(items, empty) {
  if (!items.length) return el("div", { class: "body flush" }, [el("div", { class: "empty", text: empty })]);
  return el("div", { class: "body flush" }, items);
}
