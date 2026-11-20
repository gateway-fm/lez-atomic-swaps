"""Every file a deploy/images/*/Dockerfile copies from its build context is
admitted by that context's .dockerignore and is either tracked in the
repository or staged by the build scripts. The Maker image once copied a
staged tool its .dockerignore did not admit, and the build failed with
"not found" although the tool was built and staged."""

import fnmatch
import re
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IMAGES = ROOT / 'deploy' / 'images'
STAGING_SCRIPTS = [
    ROOT / 'deploy' / 'scripts' / 'stage-assets.sh',
    ROOT / 'deploy' / 'scripts' / 'from-scratch.sh',
    ROOT / 'deploy' / 'scripts' / 'stage-basecamp-package.sh',
]


def copy_sources(dockerfile: Path) -> list[str]:
    """The build-context sources of the Dockerfile's COPY instructions."""
    text = re.sub(r'\\\n', ' ', dockerfile.read_text())
    sources = []
    for line in text.splitlines():
        words = line.split()
        if not words or words[0].upper() != 'COPY':
            continue
        arguments = [word for word in words[1:] if not word.startswith('--')]
        if any(word.startswith('--from') for word in words[1:]):
            continue
        sources.extend(arguments[:-1])
    return sources


def admitted(rules: list[str], path: str) -> bool:
    """Docker's .dockerignore reading: the last matching rule decides."""
    keep = True
    for rule in rules:
        negated = rule.startswith('!')
        pattern = rule[1:] if negated else rule
        if fnmatch.fnmatch(path, pattern) or fnmatch.fnmatch(path.split('/')[0], pattern):
            keep = negated
    return keep


def dockerignore_rules(context: Path) -> list[str]:
    path = context / '.dockerignore'
    if not path.exists():
        return []
    return [line.strip() for line in path.read_text().splitlines()
            if line.strip() and not line.strip().startswith('#')]


def tracked(context: Path, source: str) -> bool:
    listed = subprocess.run(['git', 'ls-files', '--', str(context.relative_to(ROOT))],
                            cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    prefix = str(context.relative_to(ROOT)) + '/'
    pattern = prefix + source.rstrip('/')
    return any(fnmatch.fnmatch(name, pattern) or name.startswith(pattern + '/') for name in listed)


class ImageContexts(unittest.TestCase):
    def test_every_copied_file_is_admitted_and_tracked_or_staged(self):
        staging = '\n'.join(script.read_text() for script in STAGING_SCRIPTS)
        checked = 0
        for dockerfile in sorted(IMAGES.glob('*/Dockerfile')):
            context = dockerfile.parent
            rules = dockerignore_rules(context)
            for source in copy_sources(dockerfile):
                checked += 1
                with self.subTest(image=context.name, source=source):
                    self.assertTrue(admitted(rules, source.rstrip('/')),
                                    f'{context.name}/.dockerignore does not admit {source}')
                    if tracked(context, source):
                        continue
                    literal = re.sub(r'\[(.)\]', r'\1', source.rstrip('/'))
                    # stage-basecamp-package.sh names each desk's user
                    # directory by its kind (assets/${kind}-user).
                    per_kind = (fnmatch.fnmatch(literal, 'assets/*-user')
                                and 'assets/${kind}-user' in staging)
                    # A copied glob (zec-local-poc-*) is staged when the
                    # scripts name a file it matches.
                    globbed = any(fnmatch.fnmatch(word, '*' + Path(literal).name)
                                  for word in staging.split()) if '*' in literal else False
                    self.assertTrue(per_kind or globbed or literal in staging or Path(literal).name in staging,
                                    f'{context.name}: {source} is neither tracked nor staged')
        self.assertGreater(checked, 0)

    def test_the_maker_image_admits_its_escrow_claim_probe(self):
        rules = dockerignore_rules(IMAGES / 'maker-node')
        self.assertTrue(admitted(rules, 'lez-v02-escrow-claim-probe'))
        self.assertFalse(admitted(rules, 'lez-v02-bridge-poc'))


if __name__ == '__main__':
    unittest.main()
