/**
 * TypeScript mirror of `canonical_host_identity` in
 * `src-tauri/src/printers/host_identity.rs` (spec D2). Both implementations
 * are tested against the same fixture:
 * `src-tauri/tests/fixtures/host-identity.json`.
 */

/** A strict dotted-quad IPv4 literal — no leading zeros, each octet 0-255.
 *  Rust's `IpAddr` parser rejects leading zeros too, so a string like
 *  "192.168.001.1" falls through to the literal-string branch on both
 *  sides rather than being treated as an IP address. */
const IPV4_PATTERN =
  /^(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])(\.(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])){3}$/;

/** The URL parser renders an IPv4-mapped IPv6 address in hex
 *  (`[::ffff:c0a8:101]`), but Rust's `Ipv6Addr` Display uses the dotted
 *  form (`[::ffff:192.168.1.1]`); rewrite to match Rust. */
const IPV4_MAPPED_HEX = /^\[::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})\]$/;

function ipv4MappedDotted(bracketed: string): string {
  const match = IPV4_MAPPED_HEX.exec(bracketed);
  if (!match) return bracketed;
  const high = parseInt(match[1], 16);
  const low = parseInt(match[2], 16);
  return `[::ffff:${high >> 8}.${high & 0xff}.${low >> 8}.${low & 0xff}]`;
}

/**
 * Builds the canonical `host:port` identity for a Connection, or `null`
 * when `host` is empty once trimmed. See spec D2 for the algorithm; the
 * steps below are numbered to match it.
 *
 * IPv6 canonical compression isn't built into JavaScript, so step 5 for
 * IPv6 literals borrows the URL parser: `new URL("http://[" + host +
 * "]").hostname` returns a lowercased, compressed IPv6 literal *with its
 * brackets*, which already matches the `[addr]:port` shape step 6 wants.
 */
export function canonicalHostIdentity(host: string, port: number): string | null {
  // 1. Trim ASCII whitespace.
  const trimmed = host.trim();
  // 2. Strip one pair of enclosing `[`…`]` from an IPv6 literal.
  const unbracketed =
    trimmed.startsWith("[") && trimmed.endsWith("]") ? trimmed.slice(1, -1) : trimmed;
  // 3. Lowercase.
  const lowered = unbracketed.toLowerCase();
  // 4. Strip one trailing `.`.
  const stripped = lowered.endsWith(".") ? lowered.slice(0, -1) : lowered;
  if (stripped === "") return null;

  // 5. If the result parses as an IP address, use its canonical text form.
  if (IPV4_PATTERN.test(stripped)) {
    // 6. Format as `host:port` (no re-bracketing needed for IPv4).
    return `${stripped}:${port}`;
  }
  try {
    const canonical = ipv4MappedDotted(new URL(`http://[${stripped}]`).hostname);
    // 6. `canonical` already carries its own `[...]` brackets.
    return `${canonical}:${port}`;
  } catch {
    // Not a valid IP address literal — use the literal host text.
    return `${stripped}:${port}`;
  }
}
