const vscode = require("vscode");
const { LanguageClient, TransportKind } = require("vscode-languageclient/node");

let client;

function serverCommand() {
  const configured = vscode.workspace.getConfiguration("ingot").get("lsp.path");
  if (typeof configured === "string" && configured.trim().length > 0) {
    return configured.trim();
  }
  return "ingot-lsp";
}

function activate(context) {
  const serverOptions = {
    command: serverCommand(),
    transport: TransportKind.stdio
  };
  const clientOptions = {
    documentSelector: [
      { scheme: "file", language: "ingot" },
      { scheme: "untitled", language: "ingot" }
    ],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.ing")
    }
  };

  client = new LanguageClient(
    "ingot",
    "Ingot Language Server",
    serverOptions,
    clientOptions
  );
  context.subscriptions.push(client.start());
}

function deactivate() {
  if (!client) {
    return undefined;
  }
  return client.stop();
}

module.exports = {
  activate,
  deactivate
};
