#!/usr/bin/env python3
"""Synthetic local tracker for browser testing; never contacts real trackers."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
import argparse
import time

TORRENT = b"d4:infod4:name12:rackett-test6:lengthi42eee"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/slow":
            time.sleep(3)
        if path == "/error":
            self.send_error(503, "Synthetic source unavailable")
            return
        if path == "/test.torrent":
            body, kind = TORRENT, "application/x-bittorrent"
        elif path in ("/feed", "/slow"):
            origin = f"http://127.0.0.1:{self.server.server_port}"
            rows = "".join(
                f'<item><title>{title}</title><guid>urn:fixture:{i}</guid>'
                f'<pubDate>Tue, 27 Oct 2026 12:00:00 +0000</pubDate>'
                f'<enclosure url="{origin}/test.torrent" length="{size}" '
                'type="application/x-bittorrent"/>'
                f'<torznab:attr name="seeders" value="{seeders}"/>'
                '<torznab:attr name="category" value="4000"/></item>'
                for i, (title, size, seeders) in enumerate([
                    ("Ubuntu desktop · local test fixture", 6_200_000_000, 128),
                    ("Debian netinstall · local test fixture", 680_000_000, 96),
                    ("Fedora workstation · local test fixture", 2_400_000_000, 64),
                ])
            )
            body = ('<?xml version="1.0" encoding="UTF-8"?>'
                    '<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed">'
                    '<channel><title>Rackett local test fixture</title>' + rows + '</channel></rss>').encode()
            kind = "application/rss+xml; charset=utf-8"
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=9138)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"Synthetic feed: http://127.0.0.1:{server.server_port}/feed", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.server_close()
