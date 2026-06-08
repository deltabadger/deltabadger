import { Controller } from "@hotwired/stimulus"

// Connect-wizard helper: point the "Open IBKR portal" link at the selected entity's URL.
export default class extends Controller {
  static targets = ["entity", "portal"]

  select() {
    if (this.hasEntityTarget && this.hasPortalTarget) {
      this.portalTarget.href = this.entityTarget.value
    }
  }
}
