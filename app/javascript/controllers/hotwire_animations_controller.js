import { Controller } from "@hotwired/stimulus";

// Animations for turbo streams & turbo frames:
// This code adds classes to elements that are about to be removed or added to the page.
// It uses the animation classes defined in the turbo-frame's first child data attributes.
// The data attributes must be named hw-animate-in OR hw-animate-out.
// Optional: Add the turbo stream action to the data attribute name, e.g. hw-animate-in-append.

// Connects to data-controller="hotwire-animations"
export default class extends Controller {
  connect() {
    document.addEventListener("turbo:before-stream-render", this.#handleStreamEvent);
    document.addEventListener("turbo:before-frame-render", this.#handleFrameEvent);
  }

  disconnect() {
    document.removeEventListener("turbo:before-stream-render", this.#handleStreamEvent);
    document.removeEventListener("turbo:before-frame-render", this.#handleFrameEvent);
  }

  #handleStreamEvent = (event) => {
    const actionStr = this.#capitalizeAction(event.target.action);
    this.#animateElementToRemove(event.target.target, actionStr, event);
    if (event.target.firstElementChild instanceof HTMLTemplateElement) {
      this.#animateElementToAdd(event.target.templateElement.content.firstElementChild, actionStr);
    }
  }

  #handleFrameEvent = (event) => {
    this.#animateElementToRemove(event.detail.newFrame.id, "", event);
    this.#animateElementToAdd(event.detail.newFrame, "");
  }

  #capitalizeAction(action) {
    return `${action.charAt(0).toUpperCase() + action.slice(1)}`;
  }

  #animateElementToRemove(targetId, actionStr, event) {
    const elementToRemove = document.getElementById(targetId)?.firstElementChild;
    if (elementToRemove) {
      const exitAnimationClass = this.#getAnimationClass(elementToRemove, "Out", actionStr);
      if (exitAnimationClass) {
        this.#applyExitAnimation(elementToRemove, exitAnimationClass, event);
      }
    }
  }

  #animateElementToAdd(element, actionStr) {
    const elementToAdd = element?.firstElementChild;
    if (elementToAdd) {
      const enterAnimationClass = this.#getAnimationClass(elementToAdd, "In", actionStr);
      if (enterAnimationClass) {
        elementToAdd.classList.add(enterAnimationClass);
      }
    }
  }

  #getAnimationClass(element, direction, actionStr) {
    return element.dataset[`hwAnimate${direction}${actionStr}`] || element.dataset[`hwAnimate${direction}`];
  }

  #applyExitAnimation(element, exitAnimationClass, event) {
    event.preventDefault();
    element.classList.add(exitAnimationClass);
    const { longestAnimationName, longestDuration } = this.#getLongestAnimation(element);
    element.addEventListener("animationend", function handleAnimationEnd(e) {
      if (e.animationName === longestAnimationName) {
        event.target.performAction();
        element.removeEventListener("animationend", handleAnimationEnd);
      }
    });
  }

  #getLongestAnimation(element) {
    const computedStyle = window.getComputedStyle(element);
    const animationNames = computedStyle.animationName.split(', ');
    const animationDurations = computedStyle.animationDuration.split(', ');

    let longestDuration = 0;
    let longestAnimationName = '';

    for (let i = 0; i < animationDurations.length; i++) {
      const duration = parseFloat(animationDurations[i]);
      const unit = animationDurations[i].match(/[a-z]+/i);
      const durationInMs = unit && unit[0] === 's' ? duration * 1000 : duration;

      if (durationInMs > longestDuration) {
        longestDuration = durationInMs;
        longestAnimationName = animationNames[i].trim();
      }
    }

    return { longestAnimationName, longestDuration };
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
