#!/usr/bin/env python3
"""Multi-metric Rackett versus Jackett speed comparison on synthetic fixtures.

Measures, for both engines against the same pinned definitions and the same
synthetic upstream (`examples/loadgen.rs serve`):

* startup to the first answered capabilities request, and resident memory;
* warm-cache searches (1337x, 20 rows; ACG.RIP, 100 rows with .torrent links;
  the two-source aggregate) sequentially and with 64 concurrent clients;
* uncached searches (every request a new ACG.RIP query, 20 and 500 rows), which
  exercise fetch, HTML parsing and serialization, sequentially and concurrently;
* capabilities requests.

Every response is checked for the exact item count (outside the timed interval);
errors fail the run. Single-client scenarios alternate engines request by request
(three paired runs), so CPU frequency scaling and host noise affect both alike.
Concurrent scenarios, and with --sequential every scenario, take turns per run
(R J J R R J): back-to-back requests then keep each engine's CPUs busy, while paired
requests reach an idle process each time, as sparse real-world queries do. Each engine reports the median
of its three runs, plus its own CPU time per request read from /proc. No real
trackers, accounts, downloads or production configuration are used, no image is
pulled, and the disposable Jackett container is removed afterwards.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import socket
import statistics
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent.parent


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def get(url, timeout=5):
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return response.status, response.read()


def wait_ready(url, deadline_s, process=None):
    start = time.perf_counter()
    while time.perf_counter() - start < deadline_s:
        if process is not None and process.poll() is not None:
            raise RuntimeError('process exited during startup')
        try:
            if get(url, timeout=2)[0] == 200:
                return time.perf_counter() - start
        except OSError:
            pass
        time.sleep(0.005)
    raise RuntimeError(f'not ready within {deadline_s}s: {url.split("?")[0]}')


def cpu_seconds(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')


def memory(pid):
    fields = {}
    for line in Path(f'/proc/{pid}/status').read_text().splitlines():
        key, _, value = line.partition(':')
        if key in ('VmRSS', 'VmHWM'):
            fields[key] = int(value.split()[0]) / 1024
    return {'rss_mib': round(fields['VmRSS'], 1), 'peak_rss_mib': round(fields['VmHWM'], 1)}


class Rackett:
    name = 'rackett'

    def __init__(self, binary, fixture):
        self.binary, self.fixture = str(Path(binary).resolve()), fixture
        self.dir = tempfile.TemporaryDirectory(prefix='rackett-speed-')
        self.port = free_port()
        self.process = None

    def launch(self):
        started = time.perf_counter()
        self.process = subprocess.Popen(
            [self.binary, '--data-dir', self.dir.name, '--port', str(self.port)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        wait_ready(f'http://127.0.0.1:{self.port}/health', 30, self.process)
        return started

    def start(self):
        self.launch()
        origin = f'http://127.0.0.1:{self.port}'
        self.ids = {}
        for source, path in [('1337x', 'x/'), ('acgrip', '')]:
            spec = {'name': source, 'kind': 'cardigann', 'url': f'http://127.0.0.1:{self.fixture}/{path}',
                    'request_delay_ms': 0, 'definition': {'builtin_id': source, 'settings': {}}}
            request = urllib.request.Request(origin + '/api/indexers', json.dumps(spec).encode(),
                                             {'Origin': origin, 'Content-Type': 'application/json'})
            with urllib.request.urlopen(request, timeout=10) as response:
                self.ids[source] = json.loads(response.read())['id']
        config = json.loads((Path(self.dir.name) / 'config.json').read_text())
        self.key = config['settings']['api_key']
        # Startup with both sources configured, as a restart would see it.
        self.stop()
        started = self.launch()
        wait_ready(self.url('1337x', {'t': 'caps'}), 30, self.process)
        self.startup_s = time.perf_counter() - started

    def url(self, source, params):
        target = 'all' if source == 'all' else self.ids[source]
        return f'http://127.0.0.1:{self.port}/api/v2.0/indexers/{target}/results/torznab/api?' + \
            urllib.parse.urlencode({'apikey': self.key, **params}, safe='{}')

    def memory(self):
        return memory(self.process.pid)

    def cpu(self):
        return cpu_seconds(self.process.pid)

    def stop(self):
        if self.process:
            self.process.terminate()
            self.process.wait(timeout=10)
            self.process = None

    def close(self):
        self.stop()
        self.dir.cleanup()


class Jackett:
    name = 'jackett'

    def __init__(self, image, fixture):
        self.image, self.fixture = image, fixture
        self.dir = tempfile.TemporaryDirectory(prefix='jackett-speed-')
        self.port = free_port()
        self.key = secrets.token_hex(16)
        self.container = 'rackett-speed-bench-' + uuid.uuid4().hex[:12]
        config = Path(self.dir.name) / 'Jackett'
        (config / 'Indexers').mkdir(parents=True)
        (config / 'ServerConfig.json').write_text(json.dumps({
            'Port': self.port, 'LocalBindAddress': '127.0.0.1', 'AllowExternal': False,
            'APIKey': self.key, 'UpdateDisabled': True, 'CacheEnabled': True,
            'CacheTtl': 2100, 'CacheMaxResultsPerIndexer': 1000}))
        x1337 = [
            ('sitelink', 'inputstring', f'http://127.0.0.1:{fixture}/x/'),
            ('cookieheader', 'hiddendata', ''), ('lasterror', 'hiddendata', None),
            ('filterbyuploader', 'inputstring', ''),
            ('primarydownloadlink', 'inputselect', 'magnet:'),
            ('fallbackdownloadlink', 'inputselect', '//itorrents.'),
            ('disablesorting-1337xpreventssortingsearchesduringhighserverload,whichbreakstheindexerwhenperformingakeywordsearch-disableifyougetzeroresults', 'inputbool', False),
            ('sortrequestedfromsite', 'inputselect', 'time'),
            ('orderrequestedfromsite', 'inputselect', 'desc'), ('tags', 'inputtags', '')]
        acgrip = [('sitelink', 'inputstring', f'http://127.0.0.1:{fixture}/'),
                  ('cookieheader', 'hiddendata', ''), ('lasterror', 'hiddendata', None),
                  ('tags', 'inputtags', '')]
        for name, entries in [('1337x', x1337), ('acgrip', acgrip)]:
            (config / 'Indexers' / f'{name}.json').write_text(json.dumps([
                {'id': i, 'type': kind, 'value': value} for i, kind, value in entries]))

    def start(self):
        started = time.perf_counter()
        subprocess.run(['docker', 'run', '-d', '--pull=never', '--name', self.container,
                        '--network', 'host', '-e', 'PUID=' + str(os.getuid()),
                        '-e', 'PGID=' + str(os.getgid()), '-e', 'AUTO_UPDATE=false',
                        '-v', self.dir.name + ':/config', self.image], check=True,
                       stdout=subprocess.DEVNULL, timeout=60)
        wait_ready(self.url('1337x', {'t': 'caps'}), 120)
        self.startup_s = time.perf_counter() - started
        for name in ['1337x', 'acgrip']:
            pinned = (ROOT / 'vendor/jackett/definitions' / f'{name}.yml').read_bytes()
            bundled = subprocess.check_output(['docker', 'exec', self.container, 'cat',
                                               f'/app/Jackett/Definitions/{name}.yml'], timeout=10)
            if bundled != pinned:
                raise RuntimeError(f'Jackett {name}.yml differs from the pinned definition')

    def url(self, source, params):
        return f'http://127.0.0.1:{self.port}/api/v2.0/indexers/{source}/results/torznab/api?' + \
            urllib.parse.urlencode({'apikey': self.key, **params}, safe='{}')

    def pid(self):
        top = subprocess.check_output(['docker', 'top', self.container, '-eo', 'pid,comm'], text=True)
        for line in top.splitlines()[1:]:
            pid, comm = line.split(None, 1)
            if comm.strip().lower().startswith('jackett'):
                return int(pid)
        raise RuntimeError('Jackett process not found')

    def memory(self):
        return memory(self.pid())

    def cpu(self):
        return cpu_seconds(self.pid())

    def close(self):
        subprocess.run(['docker', 'rm', '-f', self.container], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=60)
        self.dir.cleanup()


def load(loadgen, urls, expect, concurrency, requests, warmup):
    command = [loadgen, '--concurrency', str(concurrency), '--requests', str(requests),
               '--warmup', str(warmup), '--expect', str(expect)]
    for url in urls:
        command += ['--url', url]
    out = subprocess.run(command, capture_output=True, text=True, timeout=1800)
    if out.returncode:
        raise RuntimeError(f'load failed: {out.stdout.strip()} {out.stderr.strip()[-500:]}')
    result = json.loads(out.stdout)
    return result['paired'] if 'paired' in result else [result]


def timed_load(loadgen, engines, params, expect, concurrency, requests, warmup):
    """Run one load and attach each engine's CPU time per completed request."""
    source, query = params
    before = [engine.cpu() for engine in engines]
    results = load(loadgen, [engine.url(source, query) for engine in engines], expect,
                   concurrency, requests, warmup)
    for engine, start, result in zip(engines, before, results):
        total = result['requests'] + warmup
        result['cpu_us_per_request'] = (engine.cpu() - start) / total * 1e6
    return results


# (name, source, params, expected items, concurrency, requests, warm-up)
SCENARIOS = [
    ('caps', '1337x', {'t': 'caps'}, 0, 1, 3000, 200),
    ('warm 1337x 20 rows, 1 client', '1337x', {'t': 'search', 'q': 'Linux', 'limit': 20}, 20, 1, 3000, 300),
    ('warm 1337x 20 rows, 64 clients', '1337x', {'t': 'search', 'q': 'Linux', 'limit': 20}, 20, 64, 40000, 640),
    ('warm ACG.RIP 100 rows, 1 client', 'acgrip', {'t': 'search', 'q': 'r100w', 'limit': 100}, 100, 1, 2000, 200),
    ('warm ACG.RIP 100 rows, 64 clients', 'acgrip', {'t': 'search', 'q': 'r100w', 'limit': 100}, 100, 64, 20000, 640),
    ('warm aggregate 2 sources, 1 client', 'all', {'t': 'search', 'q': 'Linux', 'limit': 100}, 40, 1, 2000, 200),
    ('cold ACG.RIP 1 row, 1 client', 'acgrip', {'t': 'search', 'q': 'r1c{n}', 'limit': 20}, 1, 1, 1000, 50),
    ('cold ACG.RIP 20 rows, 1 client', 'acgrip', {'t': 'search', 'q': 'r20c{n}', 'limit': 20}, 20, 1, 1000, 50),
    ('cold ACG.RIP 100 rows, 1 client', 'acgrip', {'t': 'search', 'q': 'r100c{n}', 'limit': 100}, 100, 1, 500, 20),
    ('cold ACG.RIP 20 rows, 16 clients', 'acgrip', {'t': 'search', 'q': 'r20d{n}', 'limit': 20}, 20, 16, 4000, 64),
    ('cold ACG.RIP 500 rows, 1 client', 'acgrip', {'t': 'search', 'q': 'r500e{n}', 'limit': 500}, 500, 1, 200, 10),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--jackett-image', required=True, help='Immutable, already-installed image ID')
    parser.add_argument('--rackett-binary', required=True)
    parser.add_argument('--loadgen', default=str(ROOT / 'target/release/examples/loadgen'))
    parser.add_argument('--scale', type=float, default=1.0, help='Multiply request counts')
    parser.add_argument('--only', help='Run only scenarios whose name contains this text')
    parser.add_argument('--sequential', action='store_true',
                        help='Time single-client scenarios in back-to-back runs, not paired')
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    if not args.jackett_image.startswith('sha256:'):
        parser.error('Use an immutable installed image ID')
    fixture_port = free_port()
    fixture = subprocess.Popen([args.loadgen, 'serve', str(fixture_port)])
    engines = []
    report = {'method': __doc__.strip(), 'jackett_image': args.jackett_image,
              'single_client_timing': 'sequential' if args.sequential else 'paired',
              'definitions': ['1337x.yml', 'acgrip.yml'], 'host_load': os.getloadavg(),
              'scenarios': [], 'engines': {}}
    try:
        wait_ready(f'http://127.0.0.1:{fixture_port}/__hits', 10)
        rackett, jackett = Rackett(args.rackett_binary, fixture_port), Jackett(args.jackett_image, fixture_port)
        engines = [rackett, jackett]
        for engine in engines:
            engine.start()
            report['engines'][engine.name] = {'startup_s': round(engine.startup_s, 3),
                                              'idle_memory': engine.memory()}
            print(engine.name, report['engines'][engine.name], flush=True)
        for name, source, params, expect, concurrency, requests, warmup in SCENARIOS:
            if args.only and args.only not in name:
                continue
            requests = max(concurrency, int(requests * args.scale))
            runs = {'rackett': [], 'jackett': []}
            if concurrency == 1 and not args.sequential:
                for pair in [[rackett, jackett], [jackett, rackett], [rackett, jackett]]:
                    results = timed_load(args.loadgen, pair, (source, params), expect, 1,
                                         requests, warmup)
                    for engine, result in zip(pair, results):
                        runs[engine.name].append(result)
            else:
                for engine in [rackett, jackett, jackett, rackett, rackett, jackett]:
                    runs[engine.name] += timed_load(args.loadgen, [engine], (source, params),
                                                    expect, concurrency, requests, warmup)
            row = {'scenario': name, 'concurrency': concurrency, 'items': expect}
            for engine, results in runs.items():
                row[engine] = {
                    'p50_ms': statistics.median(r['p50_ms'] for r in results),
                    'p95_ms': statistics.median(r['p95_ms'] for r in results),
                    'p99_ms': statistics.median(r['p99_ms'] for r in results),
                    'rps': statistics.median(r['rps'] for r in results),
                    'cpu_us_per_request': statistics.median(r['cpu_us_per_request'] for r in results),
                    'response_bytes': results[0]['response_bytes'], 'runs': results}
            row['speedup_p50'] = row['jackett']['p50_ms'] / row['rackett']['p50_ms']
            row['speedup_p99'] = row['jackett']['p99_ms'] / row['rackett']['p99_ms']
            row['speedup_rps'] = row['rackett']['rps'] / row['jackett']['rps']
            row['cpu_ratio'] = row['jackett']['cpu_us_per_request'] / row['rackett']['cpu_us_per_request']
            report['scenarios'].append(row)
            print(f"{name:38} p50 {row['rackett']['p50_ms']:8.3f} / {row['jackett']['p50_ms']:8.3f} ms "
                  f"({row['speedup_p50']:5.1f}x)  p99 {row['speedup_p99']:5.1f}x  "
                  f"rps {row['rackett']['rps']:9.0f} / {row['jackett']['rps']:8.0f} ({row['speedup_rps']:5.1f}x)  "
                  f"cpu {row['rackett']['cpu_us_per_request']:7.0f} / {row['jackett']['cpu_us_per_request']:7.0f} us ({row['cpu_ratio']:5.1f}x)",
                  flush=True)
        for engine in engines:
            report['engines'][engine.name]['loaded_memory'] = engine.memory()
            print(engine.name, report['engines'][engine.name], flush=True)
        report['upstream_pages'] = int(get(f'http://127.0.0.1:{fixture_port}/__hits')[1])
    finally:
        for engine in engines:
            engine.close()
        fixture.terminate()
        fixture.wait()
    Path(args.output).write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
