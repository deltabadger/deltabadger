import { Controller } from "@hotwired/stimulus";

// Animations for turbo streams & turbo frames:
// This code adds classes to elements that are about to be removed or added to the page.
// It uses the animation classes defined in the turbo-frame's data attributes (same element where the turbo frame
// id is defined).
// The data attributes must be named hw-animate-in OR hw-animate-out.
// Optional: Add the turbo stream action to the data attribute name, e.g. hw-animate-in-append.

// This is all a bit of a hack. There are some issues with this approach:
// If unsuccessful forms should be re-rendered with errors, the animations block the correct error rendering.
// This is why the lastFormSubmissionSuccessful variable is used to skip animations if the last form submission
// was unsuccessful. However this only works if the form is targets a turbo frame (usually a modal), not turbo
// streams. The problem with turbo streams is that there can be many, and there's no way to keep the
// lastFormSubmissionSuccessful variable as false until the last stream is rendered.

// Connects to data-controller="hotwire-animations"
export default class extends Controller {
  lastFormSubmissionSuccessful = null;

  connect() {
    document.addEventListener("turbo:before-stream-render", this.#handleStreamEvent);
    document.addEventListener("turbo:before-frame-render", this.#handleFrameEvent);
    document.addEventListener("turbo:submit-end", this.#trackFormSubmission);
    document.addEventListener("click", this.#handleClickEvent, true);
    document.addEventListener("submit", this.#handleClickEvent, true);
  }

  disconnect() {
    document.removeEventListener("turbo:before-stream-render", this.#handleStreamEvent);
    document.removeEventListener("turbo:before-frame-render", this.#handleFrameEvent);
    document.removeEventListener("turbo:submit-end", this.#trackFormSubmission);
    document.removeEventListener("click", this.#handleClickEvent, true);
    document.removeEventListener("submit", this.#handleClickEvent, true);
  }

  #trackFormSubmission = (event) => {
    this.lastFormSubmissionSuccessful = event.detail.success;
  }

  #handleStreamEvent = (event) => {
    // The DCA wizard's key step answers a refused key with its frame again: the same step.
    if (event.target.target === "modal_content") this.#keepPaneScroll(event);
    const actionStr = this.#capitalizeAction(event.target.action);
    this.#animateElementToRemove(event.target.target, actionStr, event);
    if (event.target.firstElementChild instanceof HTMLTemplateElement) {
      this.#animateElementToAdd(event.target.templateElement.content.firstElementChild, actionStr);
    }
  }

  #handleFrameEvent = (event) => {
    // A same-step render of a DCA step (a refused submission, adding or removing a basket member)
    // replaces the sheet's pane, which is the scroller there: it keeps the place the user had.
    if (event.target.id === "modal_content" &&
        (this.lastFormSubmissionSuccessful === false || document.documentElement.dataset.wizardDir === "stay")) {
      this.#keepPaneScroll(event);
    }

    if (this.lastFormSubmissionSuccessful === false) {
      this.lastFormSubmissionSuccessful = null;
      return;
    }

    // The full-page dialog (Index, Signal) is the wizard's only scroller and it outlives the steps
    // rendered inside it, so a step swapped in after the user scrolled the last one would open
    // partway down. A same-step re-render ("stay") keeps its place. The DCA sheet's pane is new
    // with each step, so it starts at the top anyway.
    const stepChange = event.target.id === "modal_content" &&
      document.documentElement.dataset.wizardDir !== "stay";
    const scrollStepToTop = () => event.target.closest("dialog")?.scrollTo({ top: 0 });

    // The DCA sheet: a step change is instant, as a settings section is.
    if (event.target.id === "modal_content" && event.target.classList.contains("sheet__frame")) {
      delete document.documentElement.dataset.wizardDir;
      return;
    }

    // Wrap the wizard frame swap in a View Transition so named parts
    // (conversational sentence + step body) can morph / slide.
    if (event.target.id === "modal_content" && document.startViewTransition) {
      // A same-step re-render (adding or removing a basket member) is not a step
      // change: swap in place, no slide.
      if (document.documentElement.dataset.wizardDir === "stay") {
        delete document.documentElement.dataset.wizardDir;
        return;
      }
      const originalRender = event.detail.render;
      event.detail.render = (currentElement, newElement) => {
        // Inside the callback, so the reset is part of the transitioned state change rather than a
        // jump captured just before it.
        const transition = document.startViewTransition(() => {
          scrollStepToTop();
          originalRender(currentElement, newElement);
        });
        transition.finished.finally(() => {
          delete document.documentElement.dataset.wizardDir;
        });
      };
      return;
    }

    // No View Transition support (Firefox): reset outright.
    if (stepChange) scrollStepToTop();

    // Fallback: existing class-based animate-in/out path.
    this.#animateElementToRemove(event.detail.newFrame.id, "", event);
    this.#animateElementToAdd(event.detail.newFrame, "");
  }

  // The sheet's pane scrolls, and it is inside the frame being replaced: its scrollTop is read
  // before the render and set on the new pane after it.
  #keepPaneScroll(event) {
    const frame = document.getElementById("modal_content");
    const top = frame?.querySelector(".sheet__pane")?.scrollTop;
    if (!top) return;
    const render = event.detail.render;
    event.detail.render = async (...args) => {
      await render(...args);
      // On the next frame: Turbo focuses the step's autofocus field (the search box) after the render, which scrolls the
      // pane to it.
      requestAnimationFrame(() => {
        const pane = document.getElementById("modal_content")?.querySelector(".sheet__pane");
        if (pane) pane.scrollTop = top;
      });
    };
  }

  // Set the wizard direction flag *before* Turbo kicks off the transition, so the
  // CSS in _wizard-transitions.sass can pick the matching slide direction ("back",
  // "enter" = fade only), or skip the transition for a same-step re-render ("stay").
  #handleClickEvent = (event) => {
    const trigger = event.target.closest("[data-wizard-dir]");
    if (!trigger) return;
    document.documentElement.dataset.wizardDir = trigger.dataset.wizardDir;
  }

  #capitalizeAction(action) {
    return `${action.charAt(0).toUpperCase() + action.slice(1)}`;
  }

  #animateElementToRemove(targetId, actionStr, event) {
    const elementToRemove = document.getElementById(targetId);
    if (elementToRemove) {
      const exitAnimationClass = this.#getAnimationClass(elementToRemove, "Out", actionStr);
      if (exitAnimationClass) {
        this.#applyExitAnimation(elementToRemove, exitAnimationClass, event);
      }
    }
  }

  #animateElementToAdd(element, actionStr) {
    const elementToAdd = element;
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
