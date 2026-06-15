// The cardui screen state machine, rendered on a CC monitor via the wasmcraft
// web engine (react-reconciler, NO react-dom). Taps on the monitor become DOM
// clicks; each button posts a command to the Lua host over console.log using a
// sentinel line, and the host pushes results back in via web_message ->
// __registerHostMsg. Buttons are block-level and use the 16 CC colors.
import React, { useState, useEffect } from "react";

// React -> host: one sentinel line on the engine's stderr. The Lua kiosk parses
// "\x01CRB <json>" lines and acts on {op:...}.
const send = (cmd) => console.log("\x01CRB " + JSON.stringify(cmd));
const btn = (bg) => ({ background: bg, color: "white" });

// A waiting screen (await card / enrolling): a title, a status note that the
// host updates via {ev:"status"}, and a Cancel that bails back to locked.
function Prompt({ title, note, onCancel }) {
  return (
    <div>
      <h1 style={{ color: "yellow", textAlign: "center" }}>{title}</h1>
      <p style={{ color: "gray" }}>{note || "…"}</p>
      <button onClick={onCancel} style={btn("gray")}>[ Cancel ]</button>
    </div>
  );
}

export function CardApp() {
  const [s, setS] = useState({ screen: "locked", note: "" });

  // host -> React: register once. The engine calls __wasmcraft_message after
  // setting globalThis.__hostmsg; main.jsx parses it and calls this handler
  // inside a synchronous flushSync commit.
  useEffect(() => {
    globalThis.__registerHostMsg((m) => {
      if (m.ev === "status") setS((p) => ({ ...p, note: m.text }));
      else if (m.ev === "granted") setS({ screen: "unlocked", user: m.username, role: m.role });
      else if (m.ev === "denied") setS({ screen: "denied", note: m.reason });
      else if (m.ev === "enrolled") setS({ screen: "enrolled", user: m.username, id: m.user_id });
      else if (m.ev === "locked") setS({ screen: "locked", note: "" });
    });
  }, []);

  const cancel = () => { send({ op: "cancel" }); setS({ screen: "locked", note: "" }); };

  switch (s.screen) {
    case "awaitcard":
      return <Prompt title="Tap your card" note={s.note} onCancel={cancel} />;
    case "enrolling":
      return <Prompt title="Enrolling…" note={s.note} onCancel={cancel} />;
    case "unlocked":
      return (
        <div>
          <h1 style={{ color: "lime", textAlign: "center" }}>UNLOCKED</h1>
          <p>
            Welcome, <strong style={{ color: "cyan" }}>{s.user}</strong>
            {s.role ? " (" + s.role + ")" : ""}
          </p>
          <button onClick={() => send({ op: "opendoor" })} style={btn("green")}>[ Open Door ]</button>
          <button onClick={() => { send({ op: "lock" }); setS({ screen: "locked", note: "" }); }} style={btn("gray")}>[ Lock ]</button>
        </div>
      );
    case "denied":
      return (
        <div>
          <h1 style={{ color: "red", textAlign: "center" }}>DENIED</h1>
          <p style={{ color: "red" }}>{s.note}</p>
          <button onClick={() => { send({ op: "lock" }); setS({ screen: "locked", note: "" }); }} style={btn("gray")}>[ Back ]</button>
        </div>
      );
    case "enrolled":
      return (
        <div>
          <h1 style={{ color: "lime", textAlign: "center" }}>ENROLLED</h1>
          <p>
            {s.user} — user_id <strong style={{ color: "yellow" }}>{s.id}</strong>
          </p>
          <p style={{ color: "gray" }}>Card written. Keep the floppy safe.</p>
          <button onClick={() => { send({ op: "lock" }); setS({ screen: "locked", note: "" }); }} style={btn("gray")}>[ Back ]</button>
        </div>
      );
    default: // locked
      return (
        <div>
          <h1 style={{ color: "white", textAlign: "center" }}>🔒 LOCKED</h1>
          <button onClick={() => { send({ op: "signin" }); setS({ screen: "awaitcard", note: "" }); }} style={btn("blue")}>[ Sign in ]</button>
          <button onClick={() => { send({ op: "signup" }); setS({ screen: "enrolling", note: "" }); }} style={btn("green")}>[ Sign up ]</button>
        </div>
      );
  }
}
