#!/usr/bin/env python3
"""The paired run's byte witness (ADR-059 §6 item 2, F3).

Sits between the instrument's `serve` and the upstream. Two modes:

  mock      answers every POST itself with a deterministic responses-shaped
            body whose `usage` the MOCK AUTHORS (input_tokens = len(body)//4,
            cached 0, output 16). A delta computed from it is the fixture's
            arithmetic, never the provider's count — a dry run on this mode
            must print `basis: fixture-arithmetic` and proves plumbing only.

  forward   records the exact outbound bytes to <log-dir>/NN.off.body /
            NN.on.body (named by the run script) plus a meta.jsonl line
            (ts, path, len, sha256, sha16), then relays the request to the
            real upstream over TLS and relays the response back unchanged.

Never started on the operator's port, never reads the operator's config.
"""
import argparse
import hashlib
import http.client
import json
import ssl
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SEQ = 0
LOCK = threading.Lock()


def sha16(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()[:16]


class Handler(BaseHTTPRequestHandler):
    args: "argparse.Namespace" = argparse.Namespace()  # set in main()

    def log_message(self, format, *a):  # noqa: A002 - stdlib signature
        sys.stderr.write("%s %s\n" % (self.address_string(), format % a))

    def _record(self, body: bytes) -> int:
        global SEQ
        with LOCK:
            SEQ += 1
            seq = SEQ
        meta = {
            "seq": seq,
            "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "path": self.path,
            "len": len(body),
            "sha256": hashlib.sha256(body).hexdigest(),
            "sha16": sha16(body),
        }
        with open(self.args.log_dir + "/meta.jsonl", "a") as f:
            f.write(json.dumps(meta) + "\n")
        with open(self.args.log_dir + "/%03d.body" % seq, "wb") as f:
            f.write(body)
        return seq

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        seq = self._record(body)
        if self.args.mode == "mock":
            self._mock(seq, body)
        else:
            self._forward(body)

    def _mock(self, seq: int, body: bytes):
        try:
            model = json.loads(body).get("model", "mock")
        except Exception:
            model = "mock"
        # The mock AUTHORS the usage — this is the definition of
        # `basis: fixture-arithmetic` (ADR-059 §7(c)).
        usage = {
            "input_tokens": len(body) // 4,
            "input_tokens_details": {"cached_tokens": 0},
            "output_tokens": 16,
            "output_tokens_details": {"reasoning_tokens": 0},
        }
        resp = {
            "id": "resp-mock-%03d" % seq,
            "object": "response",
            "created_at": 1792300000 + seq,
            "status": "completed",
            "model": model,
            "output": [
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "mock reply"}]}
            ],
            "usage": usage,
        }
        payload = json.dumps(resp).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _forward(self, body: bytes):
        ctx = ssl.create_default_context()
        conn = http.client.HTTPSConnection(self.args.upstream_host,
                                           timeout=120, context=ctx)
        fwd = {k: v for k, v in self.headers.items()
               if k.lower() in ("authorization", "content-type", "accept")}
        conn.request("POST", self.path, body=body, headers=fwd)
        resp = conn.getresponse()
        data = resp.read()
        self.send_response(resp.status)
        ctype = resp.getheader("content-type")
        if ctype:
            self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--mode", choices=["mock", "forward"], required=True)
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--log-dir", required=True)
    p.add_argument("--upstream-host", default="api.deepseek.com")
    args = p.parse_args()
    Handler.args = args
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
