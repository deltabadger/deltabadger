import { Controller } from "@hotwired/stimulus"

// Carries a segmented control's choice into the form it sits in.
//
// `segmented` owns the chip and announces the choice as `segmented:change`; it renders BUTTONS,
// which post nothing. This is the whole of the join between the two — the hidden field the form
// actually submits.
//
// <div data-controller="form--segmented-field"
//      data-action="segmented:change->form--segmented-field#set">
//   <%= render 'shared/segmented', options: ... %>
//   <input type="hidden" name="format" data-form--segmented-field-target="field">
// </div>
export default class extends Controller {
  static targets = ["field"]

  set(event) {
    this.fieldTarget.value = event.detail.value
  }
}
