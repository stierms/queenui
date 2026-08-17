import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useNotices } from "./useNotices";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("useNotices", () => {
  it("expires a success receipt after five seconds", () => {
    const { result } = renderHook(() => useNotices());

    act(() => result.current.showNotice("success", "Engine added"));
    expect(result.current.notice?.message).toBe("Engine added");

    act(() => void vi.advanceTimersByTime(5000));
    expect(result.current.notice).toBeNull();
  });

  it("keeps a failure on screen indefinitely", () => {
    /*
     * The only report that an action did not happen. It used to erase itself
     * after five seconds, which is less time than an operator spends looking
     * at a board — the screen then looked exactly like a screen on which
     * nothing had gone wrong.
     */
    const { result } = renderHook(() => useNotices());

    act(() => result.current.showNotice("error", "Could not disconnect Bot"));
    act(() => void vi.advanceTimersByTime(60_000));

    expect(result.current.notice?.message).toBe("Could not disconnect Bot");
  });

  it("dismisses a failure on request", () => {
    const { result } = renderHook(() => useNotices());

    act(() => result.current.showNotice("error", "Could not save"));
    act(() => result.current.dismissNotice());

    expect(result.current.notice).toBeNull();
  });

  it("does not leave a success timer running across a failure", () => {
    // The success timer must not fire later and clear the error that replaced
    // it, which is what a single un-cleared timeout would do.
    const { result } = renderHook(() => useNotices());

    act(() => result.current.showNotice("success", "Saved"));
    act(() => void vi.advanceTimersByTime(4000));
    act(() => result.current.showNotice("error", "Then it broke"));
    act(() => void vi.advanceTimersByTime(10_000));

    expect(result.current.notice?.message).toBe("Then it broke");
  });
});
