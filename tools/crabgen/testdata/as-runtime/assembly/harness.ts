// Test harness shared by the generated vectors table (vectors.ts) and the
// hand-written edge cases (edge.ts). No closures, no exceptions: failures
// accumulate in a module-level log; main.ts exposes it to the node runner.

let log: string = "";
let nfail: i32 = 0;

export function fail(what: string, detail: string): void {
  log += "FAIL " + what + ": " + detail + "\n";
  nfail++;
}

export function failCount(): i32 {
  return nfail;
}

export function logString(): string {
  return log;
}

// expectExactErr: err must be present and TEXT-identical (the WIRE error
// strings are part of the cross-language contract).
export function expectExactErr(
  what: string,
  err: string | null,
  want: string
): void {
  if (err === null) {
    fail(what, 'expected error "' + want + '", got success');
  } else if (err != want) {
    fail(what, 'error text "' + err + '", want "' + want + '"');
  }
}

function hexNib(c: i32): i32 {
  if (c >= 0x30 && c <= 0x39) return c - 0x30; // 0-9
  if (c >= 0x61 && c <= 0x66) return c - 0x61 + 10; // a-f
  if (c >= 0x41 && c <= 0x46) return c - 0x41 + 10; // A-F
  return -1;
}

export function hexDecode(s: string): Uint8Array {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) {
    const hi = hexNib(s.charCodeAt(2 * i));
    const lo = hexNib(s.charCodeAt(2 * i + 1));
    out[i] = <u8>((hi << 4) | lo);
  }
  return out;
}

const DIGITS = "0123456789abcdef";

export function toHex(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i++) {
    s += DIGITS.charAt(b[i] >> 4);
    s += DIGITS.charAt(b[i] & 0xf);
  }
  return s;
}
