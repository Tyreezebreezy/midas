// Minimal CDP smoke probe for the disposable test-identity MIDAS instance
// (com.breezy.midas.test). Connects to WebView2's remote-debugging port,
// finds the app's own page (not a devtools-internal page), and asserts a
// few deterministic facts through the real Tauri IPC bridge. Never touches
// production or dev app-data - the caller is responsible for launching
// the app with tauri.test.conf.json before running this.
//
// Usage: node scripts/smoke-check.mjs [--port 9222] [--dev-url localhost:1420]

const args = process.argv.slice(2);
function argValue(name, fallback) {
  const i = args.indexOf(`--${name}`);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
}
const port = argValue("port", "9222");
const urlFragment = argValue("dev-url", "localhost:1420");

async function listTargets() {
  const res = await fetch(`http://127.0.0.1:${port}/json`);
  return res.json();
}

// The CDP /json endpoint can become reachable before the app's window has
// actually navigated past its initial about:blank paint, so a target's
// listed url is a snapshot, not a guarantee. Retry until a page target
// really reports the dev URL, rather than trusting the first listing.
async function findAppTarget(retries = 20, delayMs = 500) {
  let lastTargets = [];
  for (let attempt = 0; attempt < retries; attempt++) {
    lastTargets = await listTargets();
    const target = lastTargets.find(
      (t) => t.type === "page" && t.url && t.url.includes(urlFragment),
    );
    if (target) return target;
    await new Promise((r) => setTimeout(r, delayMs));
  }
  throw new Error(
    "App target not found after retries. Last targets: " +
      JSON.stringify(lastTargets.map((t) => ({ type: t.type, url: t.url }))),
  );
}

function connectWs(wsUrl) {
  const ws = new WebSocket(wsUrl);
  const pending = new Map();
  let nextId = 1;
  const ready = new Promise((resolve, reject) => {
    ws.addEventListener("open", () => resolve());
    ws.addEventListener("error", (e) =>
      reject(new Error("WS error: " + e.message)),
    );
  });
  ws.addEventListener("message", (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(JSON.stringify(msg.error)));
      else resolve(msg.result);
    }
  });
  async function send(method, params = {}) {
    await ready;
    const id = nextId++;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  }
  return { send, close: () => ws.close() };
}

async function evaluate(client, expression) {
  const wrapped = `(async () => { ${expression} })()`;
  const result = await client.send("Runtime.evaluate", {
    expression: wrapped,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(
      "Page eval error: " + JSON.stringify(result.exceptionDetails.exception),
    );
  }
  return result.result?.value ?? null;
}

function assert(condition, message) {
  if (!condition) throw new Error("SMOKE_FAIL: " + message);
  console.log("  ok - " + message);
}

const target = await findAppTarget();
const client = connectWs(target.webSocketDebuggerUrl);
await client.send("Runtime.enable");

const identity = await evaluate(client, "return window.location.href;");
assert(
  typeof identity === "string" && identity.includes(urlFragment),
  `app page loaded the dev URL (${identity})`,
);

const hasTauri = await evaluate(
  client,
  "return typeof window.__TAURI_INTERNALS__ !== 'undefined';",
);
assert(hasTauri === true, "Tauri IPC bridge is present on the page");

const schema = await evaluate(
  client,
  `const invoke = window.__TAURI_INTERNALS__.invoke;
   const accounts = await invoke('list_accounts', { includeArchived: true });
   return { accountCount: accounts.length };`,
);
assert(
  typeof schema.accountCount === "number",
  `list_accounts responded (accountCount=${schema.accountCount})`,
);

client.close();
console.log("SMOKE_CHECK=PASS");
