import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = ["typeRadio", "taxOptions", "country", "year", "downloadBtn"]

  toggle() {
    const isTaxReport = this.typeRadioTargets.find(r => r.value === "tax_report")?.checked
    this.taxOptionsTarget.classList.toggle("hidden", !isTaxReport)
    this.save()
  }

  changeCountry() { this.save() }
  changeYear() { this.save() }

  download(event) {
    event.preventDefault()
    this.closeModal()

    const isTaxReport = this.typeRadioTargets.find(r => r.value === "tax_report")?.checked

    if (isTaxReport) {
      const country = this.countryTarget.value
      const year = this.yearTarget.value
      const baseUrl = this.downloadBtnTarget.dataset.taxReportUrl

      // Async: enqueue job, inject progress flash via Turbo stream
      fetch(`${baseUrl}?country=${country}&year=${year}`, {
        headers: {
          "Accept": "text/vnd.turbo-stream.html",
          "X-CSRF-Token": this.csrfToken
        }
      }).then(response => response.text())
        .then(html => Turbo.renderStreamMessage(html))
    } else {
      window.location.href = this.downloadBtnTarget.dataset.transactionsUrl
    }
  }

  closeModal() {
    const dialog = this.element.closest("dialog")
    if (!dialog) return
    const modalController = this.application.getControllerForElementAndIdentifier(dialog, "modal--base")
    if (modalController) {
      modalController.animateOutCloseAndCleanUp()
    } else {
      dialog.close()
    }
  }

  get csrfToken() {
    return document.querySelector('meta[name="csrf-token"]')?.content
  }

  save() {
    const isTaxReport = this.typeRadioTargets.find(r => r.value === "tax_report")?.checked
    const params = new URLSearchParams({
      export_type: isTaxReport ? "tax_report" : "transactions"
    })

    if (isTaxReport) {
      params.set("country", this.countryTarget.value)
      params.set("year", this.yearTarget.value)
    }

    const url = this.element.dataset.trackerExportSaveUrl

    fetch(url, {
      method: "PATCH",
      headers: { "Content-Type": "application/x-www-form-urlencoded", "X-CSRF-Token": this.csrfToken },
      body: params
    })
  }
}
