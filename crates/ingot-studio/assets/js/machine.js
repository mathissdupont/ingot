// --- connections -----------------------------------------------------------

function renderMachine(inner) {
  inner.appendChild(head("Connections", "What this machine can reach. A key is never stored here or anywhere else Ingot writes: a provider is configured by naming the environment variable it reads."));
  if (!state.machine) return inner.appendChild(el("div", { class: "empty", text: "Looking…" }));
  const machine = state.machine;
  document.getElementById("version").textContent = machine.version;

  inner.appendChild(card("Model providers",
    rows(machine.providers.map((provider) => el("div", { class: "row" }, [
      el("span", { class: "stripe " + (provider.ready ? "pass" : provider.included ? "warn" : "idle") }),
      el("div", { class: "grow" }, [
        el("div", {}, [el("b", { text: provider.name }), el("span", { class: "sub", text: "  " + provider.protocol })]),
        el("div", { class: "sub" }, [
          provider.variables.length
            ? el("span", {}, ["reads ", ...provider.variables.map((variable, index) => el("span", {}, [
                index ? " or " : "",
                el("code", { text: variable.name }),
                el("span", { class: variable.set ? "" : "muted", text: variable.set ? " (set)" : " (not set)" }),
              ]))])
            : el("span", { text: "no credential required" }),
        ]),
        !provider.included ? el("div", { class: "fix", text: "this build does not include the " + provider.protocol + " protocol" }) : null,
        provider.declared ? el("div", { class: "fix", text: "declared by this project's ingot.toml" }) : null,
      ]),
      el("span", { class: "chip " + (provider.ready ? "pass" : "idle"), text: provider.ready ? "ready" : "not ready" }),
    ])), "This build includes no model provider.")));

  const runtime = machine.runtime;
  inner.appendChild(card("Container runtime",
    el("div", { class: "body" }, [
      runtime.available
        ? el("dl", { class: "facts" }, [
            el("dt", { text: "runtime" }), el("dd", { text: runtime.program + " " + runtime.version }),
          ])
        : el("p", { class: "muted", text: runtime.error }),
      el("div", { style: "height:8px" }),
      ...machine.images.map((image) => el("div", { class: "row", style: "padding-left:0;padding-right:0" }, [
        el("span", { class: "stripe " + (image.present ? "pass" : "warn") }),
        el("div", { class: "grow" }, [
          el("div", {}, [el("code", { text: image.reference })]),
          el("div", { class: "sub", text: image.purpose }),
        ]),
        el("span", { class: "chip " + (image.present ? "pass" : "warn"), text: image.present ? "present" : "not built" }),
      ])),
    ]),
    el("span", { class: "chip " + (runtime.available ? "pass" : "warn"), text: runtime.available ? "available" : "unavailable" })));

  inner.appendChild(card("How a provider is connected",
    el("div", { class: "body" }, [
      el("p", { class: "muted", text: "Ingot reads a credential from the environment at the moment it makes a request. Nothing writes it to a manifest, a lockfile, a package or a log, and the studio has no field to type one into. To add a service beyond the built-in three, name it and name the variable it reads:" }),
      el("pre", { class: "block", text: '[[model.provider]]\nname = "my-gateway"\nkind = "openai"\nbase-url = "https://gateway.example/v1"\napi-key-env = "MY_GATEWAY_KEY"' }),
    ])));
}
