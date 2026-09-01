import "@testing-library/jest-dom/vitest";

// Radix measures switch thumbs with ResizeObserver. jsdom does not provide the
// browser API, and the controls do not need layout measurements in unit tests.
class TestResizeObserver implements ResizeObserver {
  constructor(_callback: ResizeObserverCallback) {}
  observe(_target: Element, _options?: ResizeObserverOptions) {}
  unobserve(_target: Element) {}
  disconnect() {}
}

globalThis.ResizeObserver = TestResizeObserver;
