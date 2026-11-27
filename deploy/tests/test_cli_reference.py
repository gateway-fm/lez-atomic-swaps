"""docs/cli-reference.md names every command and every flag of both operator
CLIs: each subcommand of lez-maker-cli and lez-taker-cli (the clap enums in
their sources) has its own heading, each heading names a command that exists,
and each `--flag` the sources declare appears in the page."""
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
REFERENCE = ROOT / 'docs' / 'cli-reference.md'
MAKER = ROOT / 'crates' / 'maker-node' / 'src' / 'bin' / 'lez-maker-cli.rs'
TAKER = ROOT / 'crates' / 'taker-node' / 'src' / 'bin' / 'lez-taker-cli.rs'


def kebab(name):
    return re.sub(r'(?<=[a-z0-9])(?=[A-Z])', '-', name).lower().replace('_', '-')


def enum_body(source, name):
    start = re.search(rf'^enum {name} \{{$', source, re.M)
    if start is None:
        raise AssertionError(f'enum {name} not found')
    end = source.index('\n}\n', start.end())
    return source[start.end():end]


def variants(source, name):
    """The variants of one clap Subcommand enum, as the commands clap makes of them."""
    return [kebab(variant) for variant in
            re.findall(r'^    ([A-Z][A-Za-z0-9]*)\b', enum_body(source, name), re.M)]


def flags(source):
    """Every `--flag` the source declares with #[arg(long…)]: the field's name,
    or the name `long = "…"` gives it."""
    found = set()
    pattern = re.compile(r'#\[arg\(\s*long\b(?P<attributes>(?:(?!#\[arg\().)*?)\)\]\s*(?P<field>[a-z_0-9]+):', re.S)
    for match in pattern.finditer(source):
        renamed = re.match(r'\s*=\s*"([a-z0-9-]+)"', match.group('attributes'))
        found.add('--' + (renamed.group(1) if renamed else kebab(match.group('field'))))
    return found


def headings(text, cli):
    return set(re.findall(rf'^#+ `{cli} ([a-z -]+)`$', text, re.M))


class CliReference(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.reference = REFERENCE.read_text()
        cls.maker = MAKER.read_text()
        cls.taker = TAKER.read_text()

    def test_the_parsers_find_the_commands_and_flags(self):
        self.assertIn('configure-pair', variants(self.maker, 'Command'))
        self.assertIn('swap', variants(self.taker, 'LifecycleCommand'))
        self.assertIn('take', variants(self.taker, 'SwapCommand'))
        self.assertIn('--alert', flags(self.maker))
        self.assertNotIn('--alert-sequence', flags(self.maker))
        self.assertIn('--btc-role-root', flags(self.taker))
        self.assertIn('--funding-transaction-hex', flags(self.taker))

    def test_every_maker_command_has_a_heading_and_every_heading_a_command(self):
        self.assertEqual(headings(self.reference, 'lez-maker-cli'), set(variants(self.maker, 'Command')))

    def test_every_taker_command_has_a_heading_and_every_heading_a_command(self):
        commands = set(variants(self.taker, 'LifecycleCommand'))
        commands |= {f'swap {command}' for command in variants(self.taker, 'SwapCommand')}
        self.assertEqual(headings(self.reference, 'lez-taker-cli'), commands)

    def test_every_flag_is_named(self):
        for cli, source in (('lez-maker-cli', self.maker), ('lez-taker-cli', self.taker)):
            missing = sorted(flag for flag in flags(source)
                             if re.search(rf'`{flag}(?![a-z-])', self.reference) is None)
            self.assertEqual(missing, [], f'{cli} flags docs/cli-reference.md does not name')


if __name__ == '__main__':
    unittest.main()
