import "@testing-library/jest-dom/vitest";

// jsdom implements no layout engine and therefore no ResizeObserver. The row
// arrows observe their scroller to decide whether there is anywhere to scroll,
// so a no-op stand-in keeps that code exercisable under test.
if (!("ResizeObserver" in globalThis)) {
  class ResizeObserverStub implements ResizeObserver {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  globalThis.ResizeObserver = ResizeObserverStub;
}
