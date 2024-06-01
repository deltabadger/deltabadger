import { Controller } from "@hotwired/stimulus";

// Animations for turbo streams & turbo frames:
// This code adds classes to elements that are about to be removed or added to the page.
// It uses the animation classes defined in the turbo-frame's first child data attributes.
// The data attributes must be named hw-animate-in OR hw-animate-out.
// Optional: Add the turbo stream action to the data attribute name, e.g. hw-animate-in-append.

// Connects to data-controller="hotwire-animations"
export default class extends Controller {
  connect() {
    document.addEventListener(
      "turbo:before-stream-render",
      this.#handleStreamEvent
    );
    document.addEventListener(
      "turbo:before-frame-render",
      this.#handleFrameEvent
    );
  }

  disconnect() {
    document.removeEventListener(
      "turbo:before-stream-render",
      this.#handleStreamEvent
    );
    document.removeEventListener(
      "turbo:before-frame-render",
      this.#handleFrameEvent
    );
  }

  #handleStreamEvent(event) {
    const action = event.target.action;
    const actionStr = `${action.charAt(0).toUpperCase() + action.slice(1)}`;

    // Add a class to an element we are about to remove from the page
    const elementToRemove = document.getElementById(
      event.target.target
    )?.firstElementChild;
    if (elementToRemove) {
      let exitAnimationClass =
        elementToRemove.dataset["hwAnimateOut" + actionStr] ||
        elementToRemove.dataset["hwAnimateOut"];
      if (exitAnimationClass) {
        event.preventDefault();
        console.log("Adding stream exit animation class to", elementToRemove); // delete after testing
        elementToRemove.classList.add(exitAnimationClass);
        elementToRemove.addEventListener("animationend", () => {
          event.target.performAction();
        });
      }
    }
    // Add a class to an element we are about to add to the page
    if (event.target.firstElementChild instanceof HTMLTemplateElement) {
      const elementToAdd =
        event.target.templateElement.content.firstElementChild
          ?.firstElementChild;
      if (elementToAdd) {
        let enterAnimationClass =
          elementToAdd.dataset["hwAnimateIn" + actionStr] ||
          elementToAdd.dataset["hwAnimateIn"];
        if (enterAnimationClass) {
          console.log("Adding stream entry animation class to", elementToAdd); // delete after testing
          elementToAdd.classList.add(enterAnimationClass);
        }
      }
    }
  }

  #handleFrameEvent(event) {
    // Add a class to an element we are about to remove from the page
    let elementToRemove = document.getElementById(
      event.detail.newFrame.id
    ).firstElementChild;
    if (elementToRemove) {
      let exitAnimationClass = elementToRemove.dataset["hwAnimateOut"];
      if (exitAnimationClass) {
        event.preventDefault();
        console.log("Adding frame exit animation class to", elementToRemove); // delete after testing
        elementToRemove.classList.add(exitAnimationClass);
        elementToRemove.addEventListener("animationend", () => {
          event.target.performAction();
        });
      }
    }
    // Add a class to an element we are about to add to the page
    let elementToAdd = event.detail.newFrame.firstElementChild;
    if (elementToAdd) {
      let enterAnimationClass = elementToAdd.dataset["hwAnimateIn"];
      if (enterAnimationClass) {
        console.log("Adding frame entry animation class"); // delete after testing
        elementToAdd.classList.add(enterAnimationClass);
      }
    }
  }

  // // To use the new View Transitions (https://dev.to/nejremeslnici/how-to-use-view-transitions-in-hotwire-turbo-1kdin)
  // // https://turbo.hotwired.dev/handbook/drive#view-transitions
  // // The transitions are more efficient but present some issues:
  // // - Each animated element needs a unique id, and a css animation targeting that id -> the css can be embedded in a turbo frame with <style> tags but it's dirty
  // // - No support for multiple element animations on the same page
  // // - Browser compatibility
  // #handleNewFrameEvent(event) {
  //   console.log("handleNewFrameEvent");
  //   if (document.startViewTransition) {
  //     console.log("startViewTransition");
  //     const originalRender = event.detail.render;
  //     event.detail.render = (currentElement, newElement) => {
  //       document.startViewTransition(() =>
  //         originalRender(currentElement, newElement)
  //       );
  //     };
  //   }
  // }
}
