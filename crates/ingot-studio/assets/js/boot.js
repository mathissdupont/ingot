"use strict";

// The token arrives in the URL this process printed and stays in memory. It is
// stripped from the address bar so it does not end up in history or in a
// screenshot of one.
const TOKEN = new URLSearchParams(location.search).get("token") || "";
history.replaceState(null, "", location.pathname);

// Every value shown below is somebody's path, diagnostic or model output. The
// page builds nodes and sets textContent; there is no innerHTML anywhere, so
// none of it can become markup.
function el(tag, attrs, children) {
  const node = document.createElement(tag);
  for (const key in attrs || {}) {
    const value = attrs[key];
    if (value === null || value === undefined || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of [].concat(children || [])) {
    if (child === null || child === undefined || child === false) continue;
    node.appendChild(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return node;
}

async function api(route, options) {
  const settings = Object.assign({ headers: { "X-Ingot-Token": TOKEN } }, options || {});
  const response = await fetch("/api/" + route, settings);
  const text = await response.text();
  let parsed = null;
  try { parsed = JSON.parse(text); } catch (_) { /* a plain-text refusal */ }
  if (!response.ok) throw new Error((parsed && parsed.error) || text || response.statusText);
  return parsed;
}

const q = (params) => new URLSearchParams(params).toString();
