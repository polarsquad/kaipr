#!/usr/bin/env python3
"""kaipr inference demo: a browser chat page in front of the gateway.

Serves a small chat page and forwards /v1/* to the inference-gateway
port-forward, adding the CORS headers a browser page needs and passing
through the EPP's X-Inference-Pod pick. Stdlib only; no cluster object
is created - the page exercises the existing POST /v1/chat/completions
route on the inference-gateway Gateway.

Usage:
  python3 scripts/inference-demo.py [--port 18081]
                                    [--upstream http://127.0.0.1:18080]
                                    [--bind 127.0.0.1]
"""
import argparse
import http.client
import json
import socketserver
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>kaipr inference demo</title>
<style>
  :root {
    color-scheme: dark;
    --bg: #0f0f0f;
    --panel: #171717;
    --border: #2a2a2a;
    --fg: #e8e8e8;
    --muted: #9a9a9a;
    --accent: #ff78c9;
    --user: #232030;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; height: 100vh; display: flex; flex-direction: column;
    background: var(--bg); color: var(--fg);
    font: 15px/1.5 -apple-system, "Segoe UI", Roboto, sans-serif;
  }
  header {
    padding: 14px 20px; border-bottom: 1px solid var(--border);
    display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap;
  }
  header h1 { font-size: 15px; margin: 0; font-weight: 600; }
  header h1 span { color: var(--accent); }
  header .sub { color: var(--muted); font-size: 12px; }
  #log { flex: 1; overflow-y: auto; padding: 20px; display: flex; flex-direction: column; gap: 14px; }
  .msg { max-width: 780px; }
  .msg .who { font-size: 11px; text-transform: uppercase; letter-spacing: .08em; color: var(--muted); margin-bottom: 4px; }
  .msg .body {
    background: var(--panel); border: 1px solid var(--border);
    border-radius: 10px; padding: 10px 14px; white-space: pre-wrap;
  }
  .msg.user .body { background: var(--user); }
  .meta { font-size: 11px; color: var(--muted); margin-top: 5px; font-family: ui-monospace, "SF Mono", Menlo, monospace; }
  .meta b { color: var(--accent); font-weight: 600; }
  footer { padding: 14px 20px; border-top: 1px solid var(--border); display: flex; gap: 10px; }
  footer textarea {
    flex: 1; resize: none; height: 56px; background: var(--panel); color: var(--fg);
    border: 1px solid var(--border); border-radius: 10px; padding: 10px 12px; font: inherit;
  }
  footer textarea:focus { outline: none; border-color: var(--accent); }
  footer button {
    background: var(--accent); color: #14060f; border: 0; border-radius: 10px;
    padding: 0 22px; font-weight: 600; cursor: pointer;
  }
  footer button:disabled { opacity: .4; cursor: default; }
  #status { font-size: 12px; color: var(--muted); align-self: center; }
</style>
</head>
<body>
<header>
  <h1><span>kaipr</span> inference demo</h1>
  <div class="sub">agentgateway &rarr; llm-d Router (EPP) &rarr; vllm-sim &middot; model Qwen/Qwen3-32B (CPU simulator, canned output)</div>
</header>
<div id="log">
  <div class="msg">
    <div class="who">platform</div>
    <div class="body">Each message goes through the same path curl uses: inference-gateway &rarr; HTTPRoute &rarr; AgentgatewayBackend (token budget) &rarr; InferencePool &rarr; EPP &rarr; model-server pod. The "served by" line is the EPP's endpoint pick (X-Inference-Pod).</div>
  </div>
</div>
<footer>
  <textarea id="input" placeholder="Say something. Enter to send, Shift+Enter for a newline."></textarea>
  <button id="send">Send</button>
  <div id="status"></div>
</footer>
<script>
const log = document.getElementById('log');
const input = document.getElementById('input');
const send = document.getElementById('send');
const status = document.getElementById('status');
const esc = (s) => String(s).replace(/[&<>"']/g,
  (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

function addMsg(who, body, meta) {
  const m = document.createElement('div');
  m.className = 'msg ' + who;
  m.innerHTML = `<div class="who">${esc(who)}</div><div class="body"></div>` +
    (meta ? `<div class="meta"></div>` : '');
  m.querySelector('.body').textContent = body;
  if (meta) m.querySelector('.meta').innerHTML = meta;
  log.appendChild(m);
  log.scrollTop = log.scrollHeight;
}

async function go() {
  const content = input.value.trim();
  if (!content || send.disabled) return;
  input.value = '';
  send.disabled = true;
  addMsg('you', content);
  status.textContent = 'requesting…';
  const t0 = performance.now();
  try {
    const r = await fetch('/v1/chat/completions', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        model: 'Qwen/Qwen3-32B',
        messages: [{ role: 'user', content }],
        max_tokens: 64,
      }),
    });
    const ms = Math.round(performance.now() - t0);
    if (!r.ok) {
      const text = await r.text();
      let err;
      try { err = JSON.parse(text); } catch { err = text; }
      addMsg('gateway', `HTTP ${r.status} ${esc(r.statusText)}
${typeof err === 'string' ? err : JSON.stringify(err, null, 2)}`);
      status.textContent = '';
      return;
    }
    const j = await r.json();
    const reply = j.choices?.[0]?.message?.content ?? JSON.stringify(j, null, 2);
    const u = j.usage || {};
    const pod = esc(r.headers.get('x-inference-pod') || '-');
    addMsg('model', reply,
      `served by <b>${pod}</b> &middot; ${u.prompt_tokens ?? '?'} in / ` +
      `${u.completion_tokens ?? '?'} out tokens &middot; ${ms} ms`);
    status.textContent = '';
  } catch (e) {
    addMsg('error', String(e));
    status.textContent = '';
  }
  send.disabled = false;
  input.focus();
}

send.addEventListener('click', go);
input.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); go(); }
});
input.focus();
</script>
</body>
</html>
"""

# Upstream response headers passed through to the browser; the page
# renders only X-Inference-Pod (usage comes from the JSON body).
PASS_HEADERS = ("X-Inference-Pod", "X-Inference-Tokens-Usage")


class Handler(BaseHTTPRequestHandler):
    server_version = "kaipr-demo/1.0"

    def _cors(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers",
                         "content-type, authorization")

    def _send(self, code, body, content_type="application/json"):
        self.send_response(code)
        self._cors()
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self.send_response(204)
        self._cors()
        self.end_headers()

    def do_GET(self):
        if self.path in ("/", "/index.html"):
            body = PAGE.encode("utf-8")
            self._send(200, body, "text/html; charset=utf-8")
            return
        if self.path.startswith("/v1/"):
            self._forward()
            return
        self._send(404, json.dumps({
            "error": "not found",
            "hint": "the demo page is at /; it calls /v1/chat/completions",
        }).encode("utf-8"))

    def do_POST(self):
        if self.path.startswith("/v1/"):
            self._forward()
            return
        self._send(404, json.dumps({"error": "not found"}).encode("utf-8"))

    def _forward(self):
        upstream = self.server.upstream
        headers = {}
        for name in ("Content-Type", "Authorization"):
            if self.headers.get(name):
                headers[name] = self.headers[name]
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else None
        try:
            conn = http.client.HTTPConnection(
                upstream.hostname, upstream.port, timeout=300)
            try:
                conn.request(self.command, self.path,
                             body=body, headers=headers)
                resp = conn.getresponse()
                data = resp.read()
                self.send_response(resp.status)
                self._cors()
                self.send_header("Content-Type",
                                 resp.getheader("Content-Type",
                                                "application/json"))
                for hop in PASS_HEADERS:
                    value = resp.getheader(hop)
                    if value:
                        self.send_header(hop, value)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                if data:
                    self.wfile.write(data)
            finally:
                conn.close()
        except OSError as e:
            self._send(502, json.dumps({
                "error": "cannot reach the gateway port-forward",
                "detail": str(e),
            }).encode("utf-8"))

    def log_message(self, format, *args):
        sys.stderr.write("[inference-demo] %s\n" % (format % args))


class ThreadingServer(socketserver.ThreadingMixIn, HTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, address, handler, upstream):
        super().__init__(address, handler)
        self.upstream = upstream


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=18081,
                        help="port to serve the demo on (default: 18081)")
    parser.add_argument("--upstream", default="http://127.0.0.1:18080",
                        help="gateway port-forward base URL "
                             "(default: http://127.0.0.1:18080)")
    parser.add_argument("--bind", default="127.0.0.1",
                        help="interface to bind (default: 127.0.0.1)")
    args = parser.parse_args()

    upstream = urlparse(args.upstream)
    if upstream.scheme not in ("http", "https") or not upstream.hostname:
        print(f"ERROR: invalid --upstream {args.upstream}", file=sys.stderr)
        return 1

    # Preflight: warn early if the port-forward is not up yet, but keep
    # serving so the page can start before the forward is ready.
    probe = http.client.HTTPConnection(
        upstream.hostname, upstream.port, timeout=2)
    try:
        probe.request("GET", "/v1/chat/completions")
        probe.getresponse().read()
    except OSError:
        print(f"WARNING: no gateway port-forward at {args.upstream}; "
              f"requests will 502 until one is running", file=sys.stderr)
    finally:
        probe.close()

    server = ThreadingServer((args.bind, args.port), Handler, upstream)
    print(f"kaipr inference demo on http://{args.bind}:{args.port}/ "
          f"(forwarding /v1/* to {args.upstream})", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
