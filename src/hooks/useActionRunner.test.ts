import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useActionRunner } from "./useActionRunner";

describe("useActionRunner", () => {
  it("names the action that failed, not just the cause", async () => {
    /*
     * The backend's message describes a cause and never an intent. With
     * several actions able to run at once — the busy-key design exists for
     * exactly that — "permission denied (os error 13)" on its own does not
     * tell the operator which one just failed.
     */
    const showNotice = vi.fn();
    const { result } = renderHook(() => useActionRunner(showNotice));

    await act(async () => {
      await result.current.runAction(
        "engine-1",
        () => Promise.reject(new Error("permission denied (os error 13)")),
        "Engine removed",
        "remove Queen",
      );
    });

    expect(showNotice).toHaveBeenCalledWith(
      "error",
      "Could not remove Queen — permission denied (os error 13)",
    );
  });

  it("falls back to the bare cause when no intent is given", async () => {
    const showNotice = vi.fn();
    const { result } = renderHook(() => useActionRunner(showNotice));

    await act(async () => {
      await result.current.runAction("k", () =>
        Promise.reject(new Error("boom")),
      );
    });

    expect(showNotice).toHaveBeenCalledWith("error", "boom");
  });

  it("reports success and returns true without touching the failure copy", async () => {
    const showNotice = vi.fn();
    const { result } = renderHook(() => useActionRunner(showNotice));

    let outcome = false;
    await act(async () => {
      outcome = await result.current.runAction(
        "k",
        () => Promise.resolve(),
        "Engine removed",
        "remove Queen",
      );
    });

    expect(outcome).toBe(true);
    expect(showNotice).toHaveBeenCalledExactlyOnceWith(
      "success",
      "Engine removed",
    );
  });

  it("clears the busy key whichever way the action ends", async () => {
    const showNotice = vi.fn();
    const { result } = renderHook(() => useActionRunner(showNotice));

    await act(async () => {
      await result.current.runAction(
        "k",
        () => Promise.reject(new Error("boom")),
        undefined,
        "do the thing",
      );
    });

    expect(result.current.busy.has("k")).toBe(false);
  });
});
