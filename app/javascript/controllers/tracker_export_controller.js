import { Controller } from "@hotwired/stimulus"

export default class extends Controller {
  static targets = [
    "typeRadio", "taxOptions", "transactionsOptions", "country", "year",
    "downloadBtn", "stablecoinOption", "stablecoinCheckbox", "dateFrom", "dateTo",
    "scopeRow", "scopeRadio", "classificationPanel", "classificationRow", "saveError"
  ]

  connect() {
    this.updateVisibility()
  }

  toggle() {
    this.updateVisibility()
    this.save()
  }

  changeYear() { this.save() }

  updateVisibility() {
    const isTaxReport = this.isTaxReport
    this.taxOptionsTargets.forEach(el => el.classList.toggle("hidden", !isTaxReport))
    if (this.hasTransactionsOptionsTarget) {
      this.transactionsOptionsTarget.classList.toggle("hidden", isTaxReport)
    }
    if (this.hasScopeRowTarget) {
      this.scopeRowTarget.classList.toggle("hidden", !(isTaxReport && this.isGermany))
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
    // Narrowing the range must not rewrite the crypto choice underneath it: a user who clicks the
    // broker radio to look at it and clicks back would otherwise file their next crypto report for
    // whatever year the broker range allowed. Stash on the way in, restore on the way out.
    if (this.isBroker && this.cryptoYear === undefined) this.cryptoYear = this.yearTarget.value

    Array.from(this.yearTarget.options).forEach(option => {
      option.disabled = option.hidden = !option.dataset.scopes.split(" ").includes(scope)
    })

    if (!this.isBroker && this.cryptoYear !== undefined) {
      this.yearTarget.value = this.cryptoYear
      this.cryptoYear = undefined
    }

    if (this.yearTarget.selectedOptions[0]?.disabled) {
      const firstEnabled = Array.from(this.yearTarget.options).find(option => !option.disabled)
      if (firstEnabled) this.yearTarget.value = firstEnabled.value
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
    const row = event.target.closest('[data-tracker-export-target~="classificationRow"]')
    if (!row) return

    const category = row.querySelector('[data-role="category"]')
    const isFund = row.querySelector('[data-role="kind"]').value === "fund"
    category.parentElement.classList.toggle("hidden", !isFund)
    if (!isFund) category.value = ""

    this.saveClassification(row)
    this.updateGenerateAvailability()
  }

  // Only the row the user touched. Every other row still shows FundClassification.resolve's
  // *proposal*, and persisting a proposal would record a §20(4) election the taxpayer never made.
  //
  // Held and chained, never fire-and-forget: a lost PATCH leaves the modal showing the user's
  // choice while the report falls back to that proposal — 0% Teilfreistellung where they picked
  // 30% — on a document they sign, and `download()` could otherwise ask for the report before the
  // last save committed.
  saveClassification(row) {
    const kind = row.querySelector('[data-role="kind"]').value
    if (!kind) return

    const category = row.querySelector('[data-role="category"]').value
    // A fund whose category is still blank is rejected by design; the button is already disabled
    // while a row is incomplete, so that 422 is not a failure to report.
    const rowComplete = kind !== "fund" || Boolean(category)
    const url = this.downloadBtnTarget.dataset.fundClassificationsUrl
    const options = {
      method: "PATCH",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": this.csrfToken },
      body: JSON.stringify({
        classifications: [{ symbol: row.dataset.symbol, kind: kind, fund_category: category }]
      })
    }

    this.pendingSave = Promise.resolve(this.pendingSave)
      .then(() => fetch(url, options))
      .then(response => {
        if (!response.ok && rowComplete) this.saveFailed = true
      })
      .catch(() => { this.saveFailed = true })
      .then(() => this.updateGenerateAvailability())
  }

  get classificationsComplete() {
    if (!this.hasClassificationRowTarget) return true

    return this.classificationRowTargets.every(row => {
      const kind = row.querySelector('[data-role="kind"]').value
      const category = row.querySelector('[data-role="category"]').value
      return kind && (kind !== "fund" || category)
    })
  }

  updateGenerateAvailability() {
    const complete = !this.isBroker || (this.classificationsComplete && !this.saveFailed)
    this.downloadBtnTarget.classList.toggle("button--disabled", !complete)
    if (this.hasSaveErrorTarget) {
      this.saveErrorTarget.classList.toggle("hidden", !(this.isBroker && this.saveFailed))
    }
  }

  async download(event) {
    event.preventDefault()
    if (this.isBroker && !this.classificationsComplete) return

    // The generate request must not overtake the classification it depends on.
    await this.pendingSave
    if (this.isBroker && this.saveFailed) return

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
  get isGermany() { return this.countryTarget.value === "DE" }
  // The scope radios stay physically checked when the row hides (another country, or the plain
  // transactions export), so broker mode is what the row's visibility conditions AND the radio say.
  get isBroker() {
    return this.isTaxReport && this.isGermany && this.hasScopeRadioTarget &&
      this.scopeRadioTargets.find(r => r.checked)?.value === "broker"
  }
  get isTaxReport() { return this.exportType !== "transactions" }

  get csrfToken() {
    return document.querySelector('meta[name="csrf-token"]')?.content
  }

  save() {
    const params = new URLSearchParams({
      export_type: this.isTaxReport ? "tax_report" : "transactions"
    })

    if (this.isTaxReport) {
      params.set("country", this.countryTarget.value)
      params.set("report_scope", this.isBroker ? "broker" : "crypto")
      if (!this.isBroker) {
        // The persisted year is the crypto form's preference; the broker report has no use for it,
        // it picks its own year at generate time.
        params.set("year", this.yearTarget.value)
        if (this.hasStablecoinCheckboxTarget) {
          params.set("stablecoin_as_fiat", this.stablecoinCheckboxTarget.checked)
        }
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
