import { describe, expect, it } from "vitest";
import { canonicalEndpoint } from "./endpoint";

/**
 * The reference for every expectation here is `canonical_endpoint` in
 * `crates/queen-client/src/lib.rs`, and the first block is its own test
 * (`canonical_url_identity_and_literal_loopback_policy`) transcribed case for
 * case. If the Rust rules move, these fail — which is the point: the whole
 * value of this normalizer is that it agrees with the backend, and a normalizer
 * that quietly disagrees is worse than the raw string comparison it replaced.
 */
describe("canonicalEndpoint mirrors the Rust canonicalizer", () => {
  it("lower-cases and punycodes the host, elides port 443, trims the path", () => {
    expect(canonicalEndpoint("HTTPS://BÜCHER.example:443/base/")).toBe(
      "https://xn--bcher-kva.example/base",
    );
  });

  it("keeps a non-default port and trims the trailing slash", () => {
    expect(canonicalEndpoint("http://127.9.8.7:7788/")).toBe(
      "http://127.9.8.7:7788",
    );
  });

  it("elides port 80 for http", () => {
    expect(canonicalEndpoint("http://[::1]:80")).toBe("http://[::1]");
  });

  it("refuses cleartext to anything but a literal loopback address", () => {
    // `localhost` is a domain: it could resolve anywhere, so the Rust refuses
    // it and this must too.
    expect(canonicalEndpoint("http://localhost:7788")).toBeNull();
    expect(canonicalEndpoint("http://runner:7788")).toBeNull();
    expect(canonicalEndpoint("http://[::2]")).toBeNull();
    expect(canonicalEndpoint("http://128.0.0.1")).toBeNull();
  });

  it("accepts cleartext to every literal loopback spelling", () => {
    expect(canonicalEndpoint("http://127.0.0.1:17788")).toBe(
      "http://127.0.0.1:17788",
    );
    // WHATWG normalizes an address-shaped host, so the Rust and this agree on
    // the expanded form rather than on the operator's shorthand.
    expect(canonicalEndpoint("http://127.1")).toBe("http://127.0.0.1");
    expect(canonicalEndpoint("http://127.9.8.7")).toBe("http://127.9.8.7");
  });

  it("refuses userinfo, a query, a fragment, and a non-http scheme", () => {
    expect(canonicalEndpoint("https://user@runner")).toBeNull();
    expect(canonicalEndpoint("https://:secret@runner")).toBeNull();
    expect(canonicalEndpoint("https://runner?token=bad")).toBeNull();
    expect(canonicalEndpoint("https://runner#frag")).toBeNull();
    expect(canonicalEndpoint("ftp://runner")).toBeNull();
    expect(canonicalEndpoint("queenui://pair")).toBeNull();
  });

  it("refuses an empty query or fragment, which URL.search cannot see", () => {
    /*
     * The Rust reads a bare trailing `?` as `query() == Some("")` and refuses
     * it, but `new URL("https://runner?").search` is `""` — identical to no
     * query at all. Mirroring the Rust means catching these.
     */
    expect(canonicalEndpoint("https://runner?")).toBeNull();
    expect(canonicalEndpoint("https://runner#")).toBeNull();
  });

  it("returns null rather than throwing for input that is not a URL", () => {
    expect(canonicalEndpoint("")).toBeNull();
    expect(canonicalEndpoint("   ")).toBeNull();
    expect(canonicalEndpoint("runner-host:17789")).toBeNull();
    expect(canonicalEndpoint("https://")).toBeNull();
    expect(canonicalEndpoint("not a url at all")).toBeNull();
  });
});

describe("the spellings that used to demand pairing again", () => {
  /*
   * Each pair below is one saved endpoint and one way an operator might retype
   * it. The raw comparison in Settings called every one of these a different
   * runner.
   */
  it.each([
    ["https://runner-host:17789", "https://runner-host:17789/"],
    ["https://runner-host", "https://runner-host:443"],
    ["https://runner-host", "HTTPS://Runner-Host/"],
    ["https://runner-host/base", "https://runner-host/base/"],
    ["http://127.0.0.1:17788", "http://127.0.0.1:17788/"],
  ])("reads %s and %s as the same runner", (saved, typed) => {
    const canonical = canonicalEndpoint(saved);
    expect(canonical).not.toBeNull();
    expect(canonicalEndpoint(typed)).toBe(canonical);
  });

  it("still separates runners that really are different", () => {
    // The guard rail: normalization must not make a *different* endpoint
    // compare equal, or a stale pairing record would be used against it.
    const saved = canonicalEndpoint("https://runner-host:17789");
    expect(canonicalEndpoint("https://newrig:17789")).not.toBe(saved);
    expect(canonicalEndpoint("https://runner-host:17790")).not.toBe(saved);
    expect(canonicalEndpoint("https://runner-host")).not.toBe(saved);
    expect(canonicalEndpoint("https://runner-host:17789/base")).not.toBe(saved);
    // The path is case-sensitive on both sides; only the host is folded.
    expect(canonicalEndpoint("https://runner-host/Base")).not.toBe(
      canonicalEndpoint("https://runner-host/base"),
    );
  });

  it("is idempotent, so a canonical saved URL survives a second pass", () => {
    for (const url of [
      "HTTPS://BÜCHER.example:443/base/",
      "https://runner-host:17789",
      "http://127.0.0.1:17788",
    ]) {
      const once = canonicalEndpoint(url);
      expect(once).not.toBeNull();
      expect(canonicalEndpoint(once as string)).toBe(once);
    }
  });
});
