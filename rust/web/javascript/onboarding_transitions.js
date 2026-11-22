// The onboarding steps change like an app's views: a form or link with data-transition="<next view>" fades its view out while
// Turbo fetches the next one, and the next view's parts come in once the fade is done. The stylesheet does the moving
// (new/_onboarding_transitions.sass) off two attributes on <html>: data-leave while the old view goes, data-enter while
// the new one comes. Nothing waits on an animation but the swap itself; the new view takes input as soon as it is drawn.
const root = document.documentElement
const OUT_MS = { setup: 100, dashboard: 150, language: 100 }
let left // resolves when the old view is gone

addEventListener("turbo:submit-start", (event) => leave(event.target.dataset.transition))
addEventListener("turbo:click", (event) => leave(event.target.closest("[data-transition]")?.dataset.transition))

function leave(next) {
  if (!next) return
  // No preview of the next view from Turbo's cache: it would be drawn, and come in, twice.
  window.Turbo?.cache.clear()
  root.dataset.leave = next
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches
  left = new Promise((done) => setTimeout(done, still ? 0 : OUT_MS[next] ?? 0))
}

// A refused submission (422) or a failed request: the old view stays, so it comes back at once.
addEventListener("turbo:submit-end", (event) => {
  if (!event.detail.success) stay()
})
addEventListener("turbo:fetch-request-error", stay)

function stay() {
  delete root.dataset.leave
  left = null
}

addEventListener("turbo:before-render", async (event) => {
  if (!left) return
  event.preventDefault()
  await left
  root.dataset.enter = root.dataset.leave
  stay()
  if (root.dataset.enter === "setup") addEventListener("turbo:render", () => document.getElementById("user_email")?.focus(), { once: true })
  event.detail.resume()
  // Past the longest entrance: a later render of the same view does not play it again.
  setTimeout(() => delete root.dataset.enter, 1000)
})
