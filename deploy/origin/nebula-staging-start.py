#!/usr/bin/env python3
"""Start the isolated staging snapshot using credentials from Secret Manager."""
import json
import os
import re
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent
manifest = json.loads((root / 'manifest.json').read_text())
secrets = json.loads(subprocess.check_output([
    'gcloud', 'secrets', 'versions', 'access', 'latest',
    '--secret=patchbay-nebula-staging', '--project=general-secrets-store',
], text=True))
env = {key: value for key, value in os.environ.items() if key in ('PATH', 'HOME', 'LANG')}
env.update(secrets)
origin_token = secrets['PATCHBAY_ORIGIN_AUTH_TOKEN']
if not re.fullmatch('[a-f0-9]{64}', origin_token):
    raise ValueError('Invalid staging origin token')
runtime = Path('/run/patchbay-nebula-staging')
runtime.mkdir(mode=0o700, exist_ok=True)
origin_file = runtime / 'origin.conf'
fd = os.open(origin_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, 'w') as output:
    output.write(f'proxy_set_header X-Patchbay-Origin-Auth "{origin_token}";\n')
for name, variable in [('backend', 'BACKEND_IMAGE'), ('web', 'WEB_IMAGE'),
                       ('docs', 'DOCS_IMAGE'), ('auth-broker', 'BROKER_IMAGE')]:
    env[variable] = manifest['images'][name]
subprocess.run(['docker', 'compose', '--project-name', 'patchbay-nebula-staging',
                '-f', str(root / 'nebula-staging.compose.yml'),
                'up', '-d', '--wait', '--wait-timeout', '300'], env=env, check=True)
