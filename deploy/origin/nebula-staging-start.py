#!/usr/bin/env python3
"""Start the isolated staging snapshot using credentials from Secret Manager."""
import ipaddress
import json
import os
import re
from pathlib import Path
import subprocess
import time
from urllib.error import URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

def database_url(password):
    return 'postgres://patchbay:' + quote(password, safe='') + '@postgres:5432/patchbay?sslmode=disable'


def require_ready(response, source_sha):
    if response.status != 200:
        raise ValueError('Application is not ready')
    if response.headers.get('X-Patchbay-Commit') != source_sha:
        raise ValueError('Application source does not match the staging snapshot')


def wait_for_applications(source_sha, timeout=90):
    deadline = time.monotonic() + timeout
    for port, path in [(18211, '/readyz'), (13111, '/login'),
                       (14001, '/docs'), (14301, '/readyz')]:
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f'Staging readiness deadline exceeded on port {port}')
            try:
                request = Request(f'http://127.0.0.1:{port}{path}', headers={
                    'Host': 'patchbay-staging.nebula-spaces.com',
                    'X-Forwarded-Proto': 'https',
                })
                with urlopen(request, timeout=min(5, remaining)) as response:
                    require_ready(response, source_sha)
                break
            except (URLError, OSError, ValueError):
                time.sleep(min(1, max(0, deadline - time.monotonic())))


def main():
    root = Path(__file__).resolve().parent
    manifest = json.loads((root / 'nebula-staging.manifest.json').read_text())
    secrets = json.loads(subprocess.check_output([
        'gcloud', 'secrets', 'versions', 'access', 'latest',
        '--secret=patchbay-nebula-staging', '--project=general-secrets-store',
    ], text=True))
    env = {key: value for key, value in os.environ.items() if key in ('PATH', 'HOME', 'LANG')}
    env.update(secrets)
    env['STAGING_DATABASE_URL'] = database_url(secrets['POSTGRES_PASSWORD'])
    origin_token = secrets['PATCHBAY_ORIGIN_AUTH_TOKEN']
    if not re.fullmatch('[a-f0-9]{64}', origin_token):
        raise ValueError('Invalid staging origin token')
    for name, variable in [('backend', 'BACKEND_IMAGE'), ('web', 'WEB_IMAGE'),
                           ('docs', 'DOCS_IMAGE'), ('auth-broker', 'BROKER_IMAGE')]:
        env[variable] = manifest['images'][name]
    compose = ['docker', 'compose', '--project-name', 'patchbay-nebula-staging',
               '-f', str(root / 'nebula-staging.compose.yml')]
    # Create the project network first, then trust only its host gateway.
    env['STAGING_TRUSTED_PROXY'] = '127.0.0.1/32'
    subprocess.run([*compose, 'up', '-d', '--wait', '--wait-timeout', '60', 'postgres'],
                   env=env, check=True)
    network = json.loads(subprocess.check_output([
        'docker', 'network', 'inspect', 'patchbay-nebula-staging_default',
    ], text=True))[0]
    gateway = ipaddress.ip_address(network['IPAM']['Config'][0]['Gateway'])
    env['STAGING_TRUSTED_PROXY'] = f'{gateway}/{gateway.max_prefixlen}'
    subprocess.run([*compose, 'up', '-d', '--wait', '--wait-timeout', '240'],
                   env=env, check=True)
    wait_for_applications(manifest['source_sha'])
    runtime = Path('/run/patchbay-nebula-staging')
    runtime.mkdir(mode=0o700, exist_ok=True)
    temporary = runtime / 'origin.tmp'
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w') as output:
        output.write(f'proxy_set_header X-Patchbay-Origin-Auth "{origin_token}";\n')
    os.replace(temporary, runtime / 'origin.conf')


if __name__ == '__main__':
    main()
