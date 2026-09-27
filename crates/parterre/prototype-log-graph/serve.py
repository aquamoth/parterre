#!/usr/bin/env python3
"""PROTOTYPE, throwaway: serves the log graph-column prototype with live data from a git repository.

    python3 crates/parterre/prototype-log-graph/serve.py [REPO] [--port 8767]

then open http://127.0.0.1:8767/ . REPO defaults to the current directory. Read-only: the page
asks this server, which asks git.
"""

import argparse
import http.server
import json
import os
import subprocess
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))


def git(repo, *args):
    return subprocess.run(
        ["git", "-C", repo, "-c", "log.showSignature=false", *args], check=True, capture_output=True
    ).stdout.decode("utf-8", "replace")


def refs(repo):
    """Refs by commit hash: [{name, kind}], current branch first."""
    r = subprocess.run(["git", "-C", repo, "symbolic-ref", "-q", "HEAD"], capture_output=True)
    head = r.stdout.decode().strip() if r.returncode == 0 else ""
    out = git(repo, "for-each-ref", "--format=%(refname)%00%(objectname)%00%(*objectname)")
    by_commit = {}
    for line in out.splitlines():
        full, obj, peeled = line.split("\0")
        target = peeled or obj
        if full.startswith("refs/heads/"):
            kind, name = ("current" if full == head else "local"), full[len("refs/heads/"):]
        elif full.startswith("refs/remotes/"):
            if full.endswith("/HEAD"):
                continue
            kind, name = "remote", full[len("refs/remotes/"):]
        elif full.startswith("refs/tags/"):
            kind, name = "tag", full[len("refs/tags/"):]
        else:
            continue
        by_commit.setdefault(target, []).append({"name": name, "kind": kind})
    order = {"current": 0, "local": 1, "remote": 2, "tag": 3}
    for rs in by_commit.values():
        rs.sort(key=lambda r: (order[r["kind"]], r["name"]))
    return by_commit


def log(repo, q):
    """The log as TortoiseGit's walk options would give it. Parents are git's rewritten ones."""
    args = ["log", "--date=format-local:%Y-%m-%d %H:%M", "--format=%H%x00%P%x00%an%x00%ad%x00%s%x1e"]
    args.append("--date-order" if q.get("order") == "date" else "--topo-order")
    if q.get("first") == "1":
        args.append("--first-parent")
    if q.get("nomerges") == "1":
        args.append("--no-merges")
    if q.get("compressed") == "1":
        args.append("--simplify-by-decoration")
    if q.get("all") == "1":
        args += ["--branches", "--remotes", "--tags", "HEAD"]
    else:
        args += q.get("spec", "HEAD").split()
    out = git(repo, *args, "--")
    rows = []
    for rec in out.split("\x1e"):
        rec = rec.strip("\n")
        if not rec:
            continue
        h, p, an, ad, s = rec.split("\0")
        ps = p.split()
        if q.get("first") == "1":
            ps = ps[:1]
        rows.append([h, ps, an, ad, s])
    return rows


class Handler(http.server.SimpleHTTPRequestHandler):
    repo = "."

    def __init__(self, *a, **kw):
        super().__init__(*a, directory=HERE, **kw)

    def log_message(self, *a):
        pass

    def send_json(self, obj):
        body = json.dumps(obj, separators=(",", ":")).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        u = urllib.parse.urlparse(self.path)
        q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items()}
        try:
            if u.path == "/api/repo":
                top = git(self.repo, "rev-parse", "--show-toplevel").strip()
                abbrev = len(git(self.repo, "log", "-1", "--format=%h").strip())
                return self.send_json({"name": os.path.basename(top), "refs": refs(self.repo), "abbrev": abbrev})
            if u.path == "/api/log":
                return self.send_json(log(self.repo, q))
            if u.path == "/api/message":
                return self.send_json(git(self.repo, "log", "-1", "--format=%B", q["h"]))
        except subprocess.CalledProcessError as e:
            self.send_response(500)
            self.end_headers()
            self.wfile.write(e.stderr)
            return
        return super().do_GET()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("repo", nargs="?", default=".")
    p.add_argument("--port", type=int, default=8767)
    a = p.parse_args()
    Handler.repo = a.repo
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"PROTOTYPE log graph for {os.path.abspath(a.repo)}: http://127.0.0.1:{a.port}/", file=sys.stderr)
    srv.serve_forever()


if __name__ == "__main__":
    main()
