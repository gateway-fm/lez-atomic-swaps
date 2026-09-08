#!/usr/bin/env python3
"""Preserve Docker context/plugins in a temporary config without registry credentials."""
import argparse
import json
import os
import pathlib


def prepare(source, destination):
    source, destination = source.resolve(), destination.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise RuntimeError('temporary Docker config directory must be empty')
    destination.chmod(0o700)
    config = source/'config.json'
    original = json.loads(config.read_text()) if config.exists() else {}
    public = {key: original[key] for key in ('currentContext', 'cliPluginsExtraDirs') if key in original}
    public['auths'] = {}
    (destination/'config.json').write_text(json.dumps(public)+'\n')
    # Keep the user's daemon selection (including its context TLS files) and
    # plugin locations. No registry auth store/helper is copied or invoked.
    for name in ('contexts', 'cli-plugins'):
        path = source/name
        if path.exists():
            (destination/name).symlink_to(path, target_is_directory=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=pathlib.Path)
    args = parser.parse_args()
    prepare(pathlib.Path(os.environ.get('DOCKER_CONFIG') or pathlib.Path.home()/'.docker'), args.destination)
