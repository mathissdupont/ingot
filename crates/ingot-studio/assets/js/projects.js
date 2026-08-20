// --- projects --------------------------------------------------------------

function renderProjects(inner) {
  inner.appendChild(head("Projects", "A project is a directory with an ingot.toml. This list is a bookmark file and nothing else — every fact below is read from the directory itself."));

  const input = el("input", {
    class: "text",
    placeholder: "path to a project directory",
    spellcheck: "false",
    onkeydown: (event) => { if (event.key === "Enter") add(); },
  });
  async function add() {
    const path = input.value.trim();
    if (!path) return;
    try {
      state.projects = (await api("projects?" + q({ path }), { method: "POST" })).projects;
      input.value = "";
      state.error = null;
    } catch (error) {
      state.error = String(error.message || error);
    }
    render();
  }

  inner.appendChild(card("Add a project", el("div", { class: "body" }, [
    el("div", { class: "add" }, [input, el("button", { class: "action", text: "Add", onclick: add })]),
  ])));

  const items = state.projects.map((project) => el("div", { class: "row click", onclick: () => show("project", { path: project.path, tab: "overview", detail: null, runs: null, run: null, runId: null }) }, [
    el("span", { class: "stripe " + (project.problem ? "fail" : "idle") }),
    el("div", { class: "grow" }, [
      el("div", {}, [
        el("b", { text: project.name || "(unnamed)" }),
        project.version ? el("span", { class: "sub", text: "  " + project.version }) : null,
      ]),
      el("div", { class: "where", text: project.path }),
      project.problem ? el("div", { class: "fix", text: project.problem }) : null,
    ]),
    el("span", { class: "sub", text: project.runs === 1 ? "1 run" : project.runs + " runs" }),
    el("button", {
      class: "action quiet",
      text: "Remove",
      title: "Remove the bookmark. Nothing on disk is touched.",
      onclick: async (event) => {
        event.stopPropagation();
        try {
          state.projects = (await api("projects?" + q({ path: project.path }), { method: "DELETE" })).projects;
          if (state.path === project.path) { state.view = "projects"; state.path = null; }
        } catch (error) { state.error = String(error.message || error); }
        render();
      },
    }),
  ]));
  inner.appendChild(card("Bookmarked", rows(items, "Add the directory that holds an ingot.toml.")));
}
