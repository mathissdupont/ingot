// --- the cast ---------------------------------------------------------------
//
// One character, sixteen pixels by twenty-six, built out of rectangles.
//
// It is drawn rather than fetched for the same reason everything else here is
// inline: this page may not ask the network for anything. It is built with
// `createElementNS` rather than markup for the same reason `el` exists — an
// agent's name reaches this code, and a name is somebody else's text.

const SVGNS = "http://www.w3.org/2000/svg";

// `el`'s sibling. SVG needs its own namespace, and `className` on an SVG
// element is read-only, so `class` has to go through `setAttribute`.
function svgEl(tag, attrs, children) {
  const node = document.createElementNS(SVGNS, tag);
  for (const key in attrs || {}) {
    const value = attrs[key];
    if (value === null || value === undefined || value === false) continue;
    node.setAttribute(key, value === true ? "" : String(value));
  }
  for (const child of [].concat(children || [])) {
    if (child) node.appendChild(child);
  }
  return node;
}

// [x, y, width, height, which colour]. Read it top to bottom and it is a
// person: hair, face, eyes, chin, shirt, apron, legs, boots.
const SPRITE_BODY = [
  [5, 1, 6, 1, "hair"], [4, 2, 8, 3, "hair"],
  [4, 5, 2, 1, "hair2"], [10, 5, 2, 1, "hair2"], [6, 5, 4, 1, "skin"],
  [4, 6, 1, 3, "hair2"], [11, 6, 1, 3, "hair2"], [5, 6, 6, 3, "skin"],
  [6, 7, 1, 1, "edge"], [9, 7, 1, 1, "edge"],
  [5, 9, 6, 1, "skin"], [7, 9, 2, 1, "skin2"], [6, 10, 4, 1, "skin2"],
  [4, 11, 8, 5, "top"], [4, 11, 2, 5, "top2"], [10, 11, 2, 5, "top2"],
  [5, 16, 6, 3, "wrap"],
  [5, 19, 2, 4, "leg"], [9, 19, 2, 4, "leg"],
  [4, 23, 3, 2, "shoe"], [9, 23, 3, 2, "shoe"],
];

// The arms carry the whole performance, because at this size nothing else can.
const SPRITE_ARMS = {
  idle: [[3, 12, 1, 3, "top2"], [12, 12, 1, 3, "top2"], [3, 15, 1, 1, "skin"], [12, 15, 1, 1, "skin"]],
  asking: [[3, 11, 1, 2, "top2"], [3, 10, 1, 1, "skin"], [12, 12, 1, 3, "top2"], [12, 15, 1, 1, "skin"]],
  done: [[3, 10, 1, 2, "top2"], [12, 10, 1, 2, "top2"], [3, 9, 1, 1, "skin"], [12, 9, 1, 1, "skin"]],
  refused: [[3, 13, 1, 2, "top2"], [12, 13, 1, 2, "top2"], [3, 15, 1, 1, "skin"], [12, 15, 1, 1, "skin"]],
};

// Above the head, in the headroom the tall variant leaves. A question mark, a
// tick, and a bang — the three things a run can be in the middle of saying.
const SPRITE_MARK = {
  asking: { colour: "var(--warn)", pixels: [[13, -8, 3, 1], [12, -7, 1, 1], [16, -7, 1, 1], [16, -6, 1, 1], [15, -5, 1, 1], [14, -4, 1, 1], [14, -2, 1, 1]] },
  done: { colour: "var(--ok)", pixels: [[12, -4, 1, 1], [13, -3, 1, 1], [14, -4, 1, 1], [15, -5, 1, 1], [16, -6, 1, 1]] },
  refused: { colour: "var(--fail)", pixels: [[14, -8, 2, 4], [14, -3, 2, 1]] },
};

const SPRITE_FRAME = {
  bust: { view: "2 1 12 12", mark: false },
  card: { view: "0 0 16 26", mark: false },
  tall: { view: "-1 -9 21 36", mark: true },
};

// Which of the six palettes an agent wears, decided by its name so that it is
// the same character every time the page opens and different from its
// neighbour without anybody choosing.
function paletteFor(name) {
  let hash = 0;
  for (let index = 0; index < name.length; index += 1) {
    hash = (hash * 31 + name.charCodeAt(index)) % 99991;
  }
  return "av" + (hash % 6);
}

function pixels(list) {
  return list.map(([x, y, w, h, token]) =>
    svgEl("rect", { x: x, y: y, width: w, height: h, fill: "var(--" + token + ")" }));
}

/// One agent, as a person. `mood` is idle, asking, done or refused.
function sprite(name, variant, mood) {
  const frame = SPRITE_FRAME[variant] || SPRITE_FRAME.card;
  const arms = SPRITE_ARMS[mood] || SPRITE_ARMS.idle;
  const parts = pixels(SPRITE_BODY).concat(pixels(arms));

  const mark = frame.mark && SPRITE_MARK[mood];
  if (mark) {
    for (const [x, y, w, h] of mark.pixels) {
      parts.push(svgEl("rect", { x: x, y: y, width: w, height: h, fill: mark.colour }));
    }
  }

  return svgEl("svg", {
    class: "sprite " + variant + " " + paletteFor(name),
    viewBox: frame.view,
    role: "img",
    "aria-label": name,
  }, parts);
}
