import { Controller } from "@hotwired/stimulus"

// This controller allows showing native html5 form validation errors with custom style, inline (below each field).
// The field will include the class `invalid` when the field is invalid.
// The error message will be displayed in a div with class `form__info--invalid`.
// Errors are removed when the input changes.
// source: https://www.jorgemanrubia.com/2019/02/16/form-validations-with-html5-and-modern-rails/
//
// Where the message goes depends on the form. A stacked form wraps each field in `.form__row` and
// gets the message right under the field, as before. A conversational form has no such row: its
// fields sit inside a sentence laid out as a row flex, where a block element next to the input
// lands BESIDE the words instead of under them. Those get the message in the widget's text column.
//
// A field's message is looked up in this order:
//   data-html5-<failed constraint>-message   e.g. data-html5-range-underflow-message
//   data-html5-error-message                 the catch-all
//   field.validationMessage                  the browser's own sentence
// so a field that explains its own minimum is not made to answer a typo with that explanation.

// In `validity` order, so the first match is the most specific thing wrong with the value.
const CONSTRAINTS = [
  'valueMissing', 'typeMismatch', 'patternMismatch', 'tooLong', 'tooShort',
  'rangeUnderflow', 'rangeOverflow', 'stepMismatch', 'badInput'
]

// Connects to data-controller="form--html5-validations"
export default class extends Controller {
  // Keyed by the field itself: the multi-asset allocation inputs carry neither an id nor a name, so
  // any DOM-attribute key collides across them and lets one valid field erase another's message.
  #errors = new WeakMap()

  connect() {
    this.element.setAttribute('novalidate', true)
    this.element.addEventListener('blur', this.#onBlur, true)
    this.element.addEventListener('submit', this.#onSubmit)
    this.element.addEventListener('ajax:beforeSend', this.#onSubmit)
    this.element.addEventListener('input', this.#onInput, true)
    this.#adoptServerRenderedErrors()
  }

  disconnect() {
    this.element.removeEventListener('blur', this.#onBlur)
    this.element.removeEventListener('submit', this.#onSubmit)
    this.element.removeEventListener('ajax:beforeSend', this.#onSubmit)
    this.element.removeEventListener('input', this.#onInput)
  }

  #onInput = (event) => {
    const field = event.target
    if (this.#shouldValidateField(field)) {
      field.classList.remove('is-invalid')
      this.#removeExistingErrorMessage(field)
    }
  }

  #onBlur = (event) => {
    this.#validateField(event.target)
  }

  #onSubmit = (event) => {
    if (!this.#validateForm()) {
      event.preventDefault()
      this.#firstInvalidField.focus()
    }
  }

  #validateForm() {
    let isValid = true
    // Not using `find` because we want to validate all the fields
    this.#formFields.forEach((field) => {
      if (this.#shouldValidateField(field) && !this.#validateField(field)) isValid = false
    })
    return isValid
  }

  #validateField(field) {
    if (!this.#shouldValidateField(field))
      return true
    const isValid = field.checkValidity()
    field.classList.toggle('is-invalid', !isValid)
    this.#refreshErrorForInvalidField(field, isValid)
    return isValid
  }

  #shouldValidateField(field) {
    return typeof field.checkValidity === 'function' &&
      !field.disabled &&
      !['file', 'reset', 'submit', 'button'].includes(field.type)
  }

  // The server renders the same div for model errors (config/initializers/inline_form_errors.rb),
  // right after the field and before this controller ever runs. Take ownership of those so they
  // land where a client-side message would, and so typing clears them.
  #adoptServerRenderedErrors() {
    this.element.querySelectorAll('.form__info--invalid').forEach((error) => {
      const field = error.previousElementSibling
      if (!field || !this.#shouldValidateField(field)) return

      this.#errors.set(field, error)
      this.#placeErrorMessage(field, error)
    })
  }

  #refreshErrorForInvalidField(field, isValid) {
    this.#removeExistingErrorMessage(field)
    if (!isValid)
      this.#showErrorForInvalidField(field)
  }

  #removeExistingErrorMessage(field) {
    this.#errors.get(field)?.remove()
    this.#errors.delete(field)
  }

  #showErrorForInvalidField(field) {
    const error = this.#buildFieldError(field)
    this.#errors.set(field, error)
    this.#placeErrorMessage(field, error)
  }

  #placeErrorMessage(field, error) {
    const row = field.closest('.form__row')
    if (row) {
      field.insertAdjacentElement('afterend', error)
      return
    }

    // `.toggle-group__info` is the rule widget's text column (a column flex), so the message lands
    // under the sentence and its `.small-info`, aligned with them. Anything else falls back to the
    // form, which is itself a `.widget` column — a card-level message at the bottom.
    const column = field.closest('.toggle-group__info') || this.element
    column.appendChild(error)
  }

  // textContent, not markup: these messages interpolate exchange names and asset symbols that come
  // from exchange APIs.
  #buildFieldError(field) {
    const error = document.createElement('div')
    error.className = 'form__info form__info--invalid'
    error.textContent = this.#messageFor(field)
    return error
  }

  #messageFor(field) {
    const failed = CONSTRAINTS.find((constraint) => field.validity[constraint])
    const specific = failed &&
      field.dataset[`html5${failed[0].toUpperCase()}${failed.slice(1)}Message`]

    return specific || field.dataset.html5ErrorMessage || field.validationMessage
  }

  get #formFields() {
    return Array.from(this.element.elements).filter(field => typeof field.checkValidity === 'function')
  }

  get #firstInvalidField() {
    return this.#formFields.find(field => !field.checkValidity())
  }
}
