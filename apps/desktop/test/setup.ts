import "@testing-library/jest-dom/vitest";
import { cleanup, configure } from "@testing-library/react";
import { afterEach, beforeAll } from "vitest";

// The suite runs many jsdom files in parallel; give async lookups room so a busy machine does not cause false failures.
configure({ asyncUtilTimeout: 5000 });

beforeAll(() => {
  // jsdom has no modal <dialog> support; open/close are all the app relies on.
  HTMLDialogElement.prototype.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  sessionStorage.clear();
  window.location.hash = "";
  document.documentElement.removeAttribute("data-theme");
});
