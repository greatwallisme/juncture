#!/usr/bin/env python3
"""Simple HTTP server that reads .env and injects config into the WASM example.

Usage:
    python serve.py [--port 8080]

Reads the .env file from the project root and injects OPENAI_API_KEY,
OPENAI_BASE_URL, and OPENAI_MODEL into the HTML page as JavaScript variables.
"""

import http.server
import os
import sys
from pathlib import Path


def load_env(env_path: Path) -> dict[str, str]:
    """Parse .env file into a dict."""
    env = {}
    if not env_path.exists():
        return env
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" in line:
            key, _, value = line.partition("=")
            env[key.strip()] = value.strip()
    return env


def find_project_root() -> Path:
    """Find the juncture project root (contains .env)."""
    current = Path(__file__).resolve().parent
    for _ in range(5):
        if (current / ".env").exists():
            return current
        if (current / "Cargo.toml").exists() and (current / "crates").exists():
            return current
        current = current.parent
    return Path(__file__).resolve().parent


class Handler(http.server.SimpleHTTPRequestHandler):
    """Serve files, injecting .env config into index.html."""

    def __init__(self, *args, env_vars=None, **kwargs):
        self.env_vars = env_vars or {}
        super().__init__(*args, **kwargs)

    def do_GET(self):
        if self.path in ("/", "/index.html"):
            self.serve_index()
        else:
            super().do_GET()

    def serve_index(self):
        index_path = Path(self.directory) / "index.html"
        if not index_path.exists():
            self.send_error(404)
            return

        html = index_path.read_text()

        # Inject config as JavaScript variables before the script tag
        config_js = "<script>\n"
        for key, value in self.env_vars.items():
            config_js += f"  window.{key} = {repr(value)};\n"
        config_js += "</script>\n"

        # Insert before the module script
        html = html.replace(
            '<script type="module">',
            config_js + '<script type="module">',
        )

        # Update default values in inputs
        if "OPENAI_API_KEY" in self.env_vars:
            html = html.replace(
                'placeholder="sk-..."',
                f'placeholder="sk-..." value="{self.env_vars["OPENAI_API_KEY"]}"',
            )
        if "OPENAI_BASE_URL" in self.env_vars:
            html = html.replace(
                'value="https://api.openai.com/v1"',
                f'value="{self.env_vars["OPENAI_BASE_URL"]}"',
            )
        if "OPENAI_MODEL" in self.env_vars:
            html = html.replace(
                'value="gpt-4o-mini"',
                f'value="{self.env_vars["OPENAI_MODEL"]}"',
            )

        content = html.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(content)))
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.end_headers()
        self.wfile.write(content)


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080

    # Load .env from project root
    project_root = find_project_root()
    env_path = project_root / ".env"
    env = load_env(env_path)

    if not env.get("OPENAI_API_KEY"):
        print(f"Warning: No OPENAI_API_KEY found in {env_path}")
        print("LLM chat will not work without an API key.")
        print(f"Copy .env.example to .env and fill in your key.")
    else:
        print(f"Loaded config from {env_path}")
        print(f"  Base URL: {env.get('OPENAI_BASE_URL', 'https://api.openai.com/v1')}")
        print(f"  Model: {env.get('OPENAI_MODEL', 'gpt-4o')}")

    env_vars = {
        "OPENAI_API_KEY": env.get("OPENAI_API_KEY", ""),
        "OPENAI_BASE_URL": env.get("OPENAI_BASE_URL", "https://api.openai.com/v1"),
        "OPENAI_MODEL": env.get("OPENAI_MODEL", "gpt-4o"),
    }

    handler = lambda *args, **kwargs: Handler(*args, env_vars=env_vars, **kwargs)

    print(f"\nServing WASM example at http://localhost:{port}")
    print("Press Ctrl+C to stop.\n")

    server = http.server.HTTPServer(("", port), handler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")
        server.server_close()


if __name__ == "__main__":
    main()
