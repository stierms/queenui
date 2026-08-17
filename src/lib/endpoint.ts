/**
 * Canonical runner endpoint spelling.
 *
 * The backend canonicalizes every endpoint it stores or dials
 * (`canonical_endpoint` in `crates/queen-client/src/lib.rs`), so the URL the
 * settings view reports back is already canonical while the URL in the
 * Settings field is whatever the operator typed. Comparing the two raw strings
 * made `https://runner:443` and `https://runner/` read as a *different runner*
 * and demanded pairing again for an endpoint that was already paired.
 *
 * This mirrors the Rust rules — and only those rules — so that comparison can
 * ask "the same runner?" instead of "the same keystrokes?":
 *
 *   - WHATWG parsing (`URL` here, the `url` crate there): the scheme and host
 *     are lower-cased, an internationalized host is punycoded, and an IPv4
 *     host is normalized to dotted quad.
 *   - `http` and `https` only.
 *   - No userinfo, query, or fragment. An endpoint is a scheme, a host, a port
 *     and an optional base path; anything else is not an endpoint.
 *   - Cleartext only for a *literal* loopback address. `http://localhost` is a
 *     domain, and the Rust refuses it, so this refuses it too.
 *   - The scheme's default port is elided — 443 for https, 80 for http.
 *   - Trailing slashes are trimmed from the path, while the rest of the path
 *     is preserved verbatim (it is case-sensitive on both sides).
 *
 * Anything the Rust would reject yields `null`, which a caller has to read as
 * "not a known endpoint" rather than as a match — a URL that cannot be
 * canonicalized is exactly the one that cannot be assumed to be the runner
 * already on file.
 *
 * Comparison only. Nothing here is sent anywhere: the backend canonicalizes
 * and re-validates whatever it is given, and its verdict is the one that
 * counts.
 */
export function canonicalEndpoint(url: string): string | null {
  const raw = url.trim();
  /*
   * The Rust refuses `query().is_some()` and `fragment().is_some()`, and both
   * are `Some("")` for a bare trailing `?` or `#` — an *empty* query, not an
   * absent one. `URL.search` and `URL.hash` collapse those two cases to `""`,
   * so the delimiter is checked in the input instead: under WHATWG parsing a
   * literal `?` or `#` always opens a query or a fragment.
   */
  if (raw.includes("?") || raw.includes("#")) return null;
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return null;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
  // The Rust also guards `host_str().is_none()`; for these two schemes WHATWG
  // requires a non-empty host, so `new URL` has already thrown by here.
  if (parsed.username !== "" || parsed.password !== "") return null;
  if (parsed.protocol === "http:" && !isLiteralLoopback(parsed.hostname)) {
    return null;
  }
  // `URL.host` drops the scheme's default port during parsing, which is the
  // elision the Rust performs explicitly with `set_port(None)`.
  return `${parsed.protocol}//${parsed.host}${parsed.pathname.replace(/\/+$/, "")}`;
}

/**
 * `Ipv4Addr::is_loopback` (127.0.0.0/8) or `Ipv6Addr::is_loopback` (`::1`) for
 * a host the parser resolved to an address. A host that is still a domain —
 * `localhost` included — is not one, which is the entire point of the rule:
 * the name could resolve anywhere, so cleartext to it is not safe by
 * inspection.
 */
function isLiteralLoopback(hostname: string): boolean {
  // WHATWG serializes an IPv6 host bracketed, compressed and lower-cased, so
  // every spelling of the loopback address arrives here as exactly `[::1]`.
  if (hostname.startsWith("[")) return hostname === "[::1]";
  const octets = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(hostname);
  // A host that serializes as a dotted quad *is* an IPv4 address: WHATWG parses
  // any address-shaped host into this form, and leaves domains alone.
  return octets !== null && Number(octets[1]) === 127;
}
