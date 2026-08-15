import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = [
    "typeRadio", "taxOptions", "transactionsOptions", "country", "year",
    "downloadBtn", "stablecoinOption", "stablecoinCheckbox", "dateFrom", "dateTo",
    "classificationPanel", "classificationRow", "kindSelect", "categorySelect"
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
    if (this.hasClassificationPanelTarget) {
      this.classificationPanelTarget.classList.toggle("hidden", !this.isBroker)
    }
    this.updateScope()
    this.updateStablecoinVisibility()
    this.updateButtonLabel()
    this.updateGenerateAvailability()
  }

  updateScope() {
    const scope = this.isBroker ? "broker" : "crypto"
    Array.from(this.yearTarget.options).forEach(option => {
      option.disabled = option.hidden = !option.dataset.scopes.split(" ").includes(scope)
    })

    if (this.yearTarget.selectedOptions[0]?.disabled) {
      const firstEnabled = Array.from(this.yearTarget.options).find(option => !option.disabled)
      if (firstEnabled) this.yearTarget.value = firstEnabled.value
    }

    if (this.isBroker) {
      this.countryTarget.value = "DE"
      this.countryTarget.disabled = true
    } else {
      this.countryTarget.disabled = false
    }
  }

  updateStablecoinVisibility() {
    if (!this.hasStablecoinOptionTarget) return
    const selected = this.countryTarget.selectedOptions[0]
    const ambiguous = !this.isBroker && selected?.dataset.stablecoinAmbiguous === "true"
    this.stablecoinOptionTarget.classList.toggle("hidden", !this.isTaxReport || !ambiguous)
  }

  updateButtonLabel() {
    const btn = this.downloadBtnTarget
    btn.textContent = this.isTaxReport ? btn.dataset.labelGenerate : btn.dataset.labelDownload
  }

  changeClassification(event) {
    if (!this.hasClassificationRowTarget || !this.hasKindSelectTarget || !this.hasCategorySelectTarget) return

    const row = event.target.closest('[data-tracker-export-target~="classificationRow"]')
    if (!row) return

    const kindSelect = row.querySelector('[data-tracker-export-target~="kindSelect"]')
    const categorySelect = row.querySelector('[data-tracker-export-target~="categorySelect"]')
    const isFund = kindSelect.value === "fund"
    categorySelect.parentElement.classList.toggle("hidden", !isFund)
    if (!isFund) categorySelect.value = ""

    this.saveClassifications()
    this.updateGenerateAvailability()
  }

  saveClassifications() {
    if (!this.hasClassificationRowTarget || !this.hasKindSelectTarget || !this.hasCategorySelectTarget) return

    const rows = this.classificationRowTargets.map(row => {
      const kind = row.querySelector('[data-tracker-export-target~="kindSelect"]').value
      const category = row.querySelector('[data-tracker-export-target~="categorySelect"]').value
      if (!kind) return null
      return { symbol: row.dataset.symbol, kind: kind, fund_category: category }
    }).filter(row => row)

    fetch(this.downloadBtnTarget.dataset.fundClassificationsUrl, {
      method: "PATCH",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": this.csrfToken },
      body: JSON.stringify({ classifications: rows })
    })
  }

  get classificationsComplete() {
    if (!this.hasClassificationRowTarget) return true
    if (!this.hasKindSelectTarget || !this.hasCategorySelectTarget) return false

    return this.classificationRowTargets.every(row => {
      const kind = row.querySelector('[data-tracker-export-target~="kindSelect"]').value
      const category = row.querySelector('[data-tracker-export-target~="categorySelect"]').value
      return kind && (kind !== "fund" || category)
    })
  }

  updateGenerateAvailability() {
    const complete = !this.isBroker || this.classificationsComplete
    this.downloadBtnTarget.classList.toggle("button--disabled", !complete)
  }

  download(event) {
    event.preventDefault()
    if (this.isBroker && !this.classificationsComplete) return

    this.closeModal()

    if (this.isTaxReport) {
      const year = this.yearTarget.value
      const baseUrl = this.downloadBtnTarget.dataset.taxReportUrl
      let query
      if (this.isBroker) {
        query = `country=DE&year=${year}&report_scope=broker`
      } else {
        const country = this.countryTarget.value
        const stablecoinAsFiat = this.hasStablecoinCheckboxTarget && this.stablecoinCheckboxTarget.checked
        query = `country=${country}&year=${year}&stablecoin_as_fiat=${stablecoinAsFiat}`
      }

      fetch(`${baseUrl}?${query}`, {
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

  get exportType() { return this.typeRadioTargets.find(r => r.checked)?.value }
  get isBroker() { return this.exportType === "broker_tax_report" }
  get isTaxReport() { return this.exportType !== "transactions" }

  get csrfToken() {
    return document.querySelector('meta[name="csrf-token"]')?.content
  }

  save() {
    const params = new URLSearchParams({
      export_type: this.isTaxReport ? "tax_report" : "transactions"
    })

    if (this.isTaxReport) {
      params.set("country", this.isBroker ? "DE" : this.countryTarget.value)
      params.set("year", this.yearTarget.value)
      params.set("report_scope", this.isBroker ? "broker" : "crypto")
      if (!this.isBroker && this.hasStablecoinCheckboxTarget) {
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
