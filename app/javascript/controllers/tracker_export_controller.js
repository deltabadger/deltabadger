import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = [
    "typeRadio", "taxOptions", "transactionsOptions", "country", "year",
    "downloadBtn", "stablecoinOption", "stablecoinCheckbox", "dateFrom", "dateTo"
  ]

  connect() {
    this.updateVisibility()
  }

  toggle() {
    this.updateVisibility()
    this.save()
  }

  changeCountry() {
    this.updateStablecoinVisibility()
    this.save()
  }

  changeYear() { this.save() }

  updateVisibility() {
    const isTaxReport = this.isTaxReport
    this.taxOptionsTarget.classList.toggle("hidden", !isTaxReport)
    if (this.hasTransactionsOptionsTarget) {
      this.transactionsOptionsTarget.classList.toggle("hidden", isTaxReport)
    }
    this.updateStablecoinVisibility()
    this.updateButtonLabel()
  }

  updateStablecoinVisibility() {
    if (!this.hasStablecoinOptionTarget) return
    const selected = this.countryTarget.selectedOptions[0]
    const ambiguous = selected?.dataset.stablecoinAmbiguous === "true"
    this.stablecoinOptionTarget.classList.toggle("hidden", !this.isTaxReport || !ambiguous)
  }

  updateButtonLabel() {
    const btn = this.downloadBtnTarget
    btn.textContent = this.isTaxReport ? btn.dataset.labelGenerate : btn.dataset.labelDownload
  }

  download(event) {
    event.preventDefault()
    this.closeModal()

    if (this.isTaxReport) {
      const country = this.countryTarget.value
      const year = this.yearTarget.value
      const stablecoinAsFiat = this.hasStablecoinCheckboxTarget && this.stablecoinCheckboxTarget.checked
      const baseUrl = this.downloadBtnTarget.dataset.taxReportUrl

      fetch(`${baseUrl}?country=${country}&year=${year}&stablecoin_as_fiat=${stablecoinAsFiat}`, {
        headers: {
          "Accept": "text/vnd.turbo-stream.html",
          "X-CSRF-Token": this.csrfToken
        }
      }).then(response => response.text())
        .then(html => Turbo.renderStreamMessage(html))
    } else {
      const params = new URLSearchParams()
      if (this.hasDateFromTarget && this.dateFromTarget.value) params.set("from", this.dateFromTarget.value)
      if (this.hasDateToTarget && this.dateToTarget.value) params.set("to", this.dateToTarget.value)
      const base = this.downloadBtnTarget.dataset.transactionsUrl
      window.location.href = params.toString() ? `${base}?${params}` : base
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

  get isTaxReport() {
    return this.typeRadioTargets.find(r => r.value === "tax_report")?.checked
  }

  get csrfToken() {
    return document.querySelector('meta[name="csrf-token"]')?.content
  }

  save() {
    const params = new URLSearchParams({
      export_type: this.isTaxReport ? "tax_report" : "transactions"
    })

    if (this.isTaxReport) {
      params.set("country", this.countryTarget.value)
      params.set("year", this.yearTarget.value)
      if (this.hasStablecoinCheckboxTarget) {
        params.set("stablecoin_as_fiat", this.stablecoinCheckboxTarget.checked)
      }
    }

    const url = this.element.dataset.trackerExportSaveUrl

    fetch(url, {
      method: "PATCH",
      headers: { "Content-Type": "application/x-www-form-urlencoded", "X-CSRF-Token": this.csrfToken },
      body: params
    })
  }
}
