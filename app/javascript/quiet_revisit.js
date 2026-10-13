import { morphElements } from "@hotwired/turbo";

// A revisit of a bot page renders twice: Turbo's cached copy first (the preview), then the page the
// server answers with. Replacing the preview with that answer threw away everything the preview had
// just built — the chart above all, drawn again from scratch. This render alone morphs instead: the
// one answering the preview on screen, of the same visit and the same bot. Every other render —
// another page, a refresh, a first visit, Back — is Turbo's own replace.
//
// Inside the morph, an element whose behaviour is set when it connects (a Stimulus controller, a
// frame that still has to load, a stream subscription) is either kept, when the server sent it
// exactly as it stands, or swapped whole for the new one, as a replace would. So no controller has
// to know about morphing; the chart chooses to (bot/chart_controller.js).

let visits = 0;
// The preview this page's next render may morph: { visit, bot }. Consumed by that render, and cleared
// by any other visit — a navigation that overtakes the preview must not morph into it.
let preview = null;
let swaps = null;

// The bot a body shows, by the id of its columns (dom_id(bot, :columns)); null off a bot page.
const botOf = (body) => body.querySelector("turbo-frame#bot .columns[id]")?.id ?? null;

addEventListener("turbo:visit", () => {
  visits += 1;
  preview = null;
});

addEventListener("turbo:render", () => {
  const bot = botOf(document.body);
  preview = document.documentElement.hasAttribute("data-turbo-preview") && bot ? { visit: visits, bot } : null;
});

addEventListener("turbo:before-render", (event) => {
  const eligible = preview;
  preview = null;
  // Turbo marks <html> for an incoming preview before this event: a preview never morphs.
  if (document.documentElement.hasAttribute("data-turbo-preview")) return;
  if (!eligible || eligible.visit !== visits || botOf(event.detail.newBody) !== eligible.bot) return;

  // The renderer still runs Bardo around `render`, and Bardo moving permanent nodes behind a morph
  // leaves a clone beside the original. With their twins unmarked its map is empty; the morph itself
  // never touches a permanent node, as in Turbo's own morphing renderer.
  for (const node of event.detail.newBody.querySelectorAll("[id][data-turbo-permanent]")) {
    if (document.getElementById(node.id)?.hasAttribute("data-turbo-permanent")) node.removeAttribute("data-turbo-permanent");
  }
  event.detail.render = (current, next) => {
    swaps = [];
    try {
      morphElements(current, next);
    } finally {
      const queued = swaps;
      swaps = null;
      for (const [node, replacement] of queued) node.replaceWith(replacement);
    }
  };
});

// Bubbling: an element that morphs itself has already said so (preventDefault) on its own listener.
addEventListener("turbo:before-morph-element", (event) => {
  const { currentElement: node, newElement: next } = event.detail;
  if (!swaps || event.defaultPrevented || !next) return;

  const verdict = settle(node, next);
  if (verdict === "morph") return;

  event.preventDefault();
  if (verdict === "swap") swaps.push([node, next]);
});

// "morph": an ordinary element. "keep": leave it as it stands. "swap": replace it after the morph.
function settle(node, next) {
  // The root: morphed, so <body>'s own controllers stay connected (they only listen on the document).
  if (node === document.body) return "morph";
  if (node.localName === "turbo-cable-stream-source") {
    // `connected` is the subscription's state, not its identity.
    return sameAttributes(node, next, ["connected"]) ? "keep" : "swap";
  }
  if (node.localName === "turbo-frame") {
    const src = next.getAttribute("src");
    // The server's frame has nothing to load: a src left behind by a past navigation (the bot
    // switcher, a chart that came through its lazy frame) is history, and dropping it loads nothing.
    if (!src) return node.hasAttribute("data-controller") ? swapUnlessEqual(node, next) : "morph";
    // Already showing what that src loads: keep it rather than fall back to the placeholder.
    if (node.hasAttribute("complete") && sameURL(node.getAttribute("src"), src)) return "keep";
    return "swap";
  }
  if (node.hasAttribute("data-controller")) return swapUnlessEqual(node, next);
  return "morph";
}

const swapUnlessEqual = (node, next) => (node.isEqualNode(next) ? "keep" : "swap");

function sameAttributes(node, next, ignored) {
  const names = (element) => element.getAttributeNames().filter((name) => !ignored.includes(name)).sort();
  const ours = names(node);
  const theirs = names(next);
  return ours.length === theirs.length && ours.every((name, i) => name === theirs[i] && node.getAttribute(name) === next.getAttribute(name));
}

function sameURL(a, b) {
  if (!a) return false;
  return new URL(a, location.href).href === new URL(b, location.href).href;
}
