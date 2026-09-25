import { Controller } from "@hotwired/stimulus"

// The split modal's proceeds rows. Outside the split_modal frame, so what it holds survives the frame's
// re-fetches: the bots the user answered No for. Yes is the real redeploy, so its answer lives on the
// server — the frame is re-fetched at once, and every 3 s while a reinvestment is pending, until the row
// disappears (all spent) or asks again (spent in part, or the job did nothing).
//
// Split is live only when no row is pending or halted and every row still asking was answered No; the
// No answers go out with it as keep_ids[]. The server checks all of it again.
export default class extends Controller {
  static targets = ["form"]
  static values = { url: String }

  connect() {
    this.kept = new Set()
    this.onLoad = () => this.sync()
    this.onSubmitEnd = this.answered.bind(this)
    this.element.addEventListener("turbo:frame-load", this.onLoad)
    this.element.addEventListener("turbo:submit-end", this.onSubmitEnd)
    this.sync()
  }

  disconnect() {
    clearTimeout(this.timer)
    this.element.removeEventListener("turbo:frame-load", this.onLoad)
    this.element.removeEventListener("turbo:submit-end", this.onSubmitEnd)
  }

  keep({ params: { id } }) {
    this.kept.add(String(id))
    this.sync()
  }

  answered(event) {
    if (event.target.matches("[data-split-answer]") && event.detail.success) this.reload()
  }

  reload() {
    clearTimeout(this.timer)
    const frame = this.element.querySelector("turbo-frame#split_modal")
    if (!frame) return
    if (frame.getAttribute("src") === this.urlValue) frame.reload()
    else frame.src = this.urlValue
  }

  sync() {
    clearTimeout(this.timer)
    const rows = Array.from(this.element.querySelectorAll("[data-split-row]"))
    rows.forEach(row => {
      if (row.dataset.splitRow !== "offer") return
      const kept = this.kept.has(row.dataset.botId)
      row.querySelector("[data-split-actions]").hidden = kept
      row.querySelector("[data-split-kept]").hidden = !kept
    })

    if (this.hasFormTarget) {
      const open = rows.some(row => row.dataset.splitRow !== "offer" || !this.kept.has(row.dataset.botId))
      this.formTarget.querySelectorAll("input[name='keep_ids[]']").forEach(input => input.remove())
      rows.filter(row => this.kept.has(row.dataset.botId)).forEach(row => {
        const input = document.createElement("input")
        input.type = "hidden"
        input.name = "keep_ids[]"
        input.value = row.dataset.botId
        this.formTarget.appendChild(input)
      })
      this.formTarget.querySelectorAll("button").forEach(button => { button.disabled = open })
    }

    if (rows.some(row => row.dataset.splitRow === "pending")) this.timer = setTimeout(() => this.reload(), 3000)
  }
}
