import { Controller } from "@hotwired/stimulus";

// Connects to data-controller="turbo-streams-animations"
export default class extends Controller {
  connect() {
    document.addEventListener(
      "turbo:before-stream-render",
      this.handleStreamEvent
    );
  }

  disconnect() {
    document.removeEventListener(
      "turbo:before-stream-render",
      this.handleStreamEvent
    );
  }

  handleStreamEvent(event) {

    // Animations for turbo streams:
    // This code adds classes to elements that are about to be removed or added to the page
    // based on their "data-stream-[action]-class" attributes. Allowed actions (tested):
    // - append: data-stream-append-class: "class-in"
    // - prepend: data-stream-prepend-class: "class-in"
    // - update: data-stream-update-class: "class-in class-out"
    // - remove: data-stream-remove-class: "class-out"

    const action = event.target.action;
    const streamActionClass = `stream${
      action.charAt(0).toUpperCase() + action.slice(1)
    }Class`;
    // Add a class to an element we are about to remove from the page
    let elementToRemove = document.getElementById(
      event.target.target
    ).firstElementChild;
    if (elementToRemove) {
      let streamExitClasses = elementToRemove.dataset[streamActionClass];
      if (streamExitClasses) {
        event.preventDefault();
        let streamExitClass =
          streamExitClasses.split(" ")[streamExitClasses.split(" ").length - 1];
        elementToRemove.classList.add(streamExitClass);
        elementToRemove.addEventListener("animationend", () => {
          event.target.performAction();
        });
      }
    }
    // Add a class to an element we are about to add to the page
    if (event.target.firstElementChild instanceof HTMLTemplateElement) {
      let elementToAdd =
        event.target.templateElement.content.firstElementChild
          ?.firstElementChild;
      if (elementToAdd) {
        let enterAnimationClasses = elementToAdd.dataset[streamActionClass];
        if (enterAnimationClasses) {
          let enterAnimationClass = enterAnimationClasses.split(" ")[0];
          elementToAdd.classList.add(enterAnimationClass);
        }
      }
    }
  }
}
