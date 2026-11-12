"""The role Node payload packages carry the same executables on every
platform: package-role-node.sh (Linux, from the images' staged executables)
and package-role-node-darwin.sh (macOS on Apple silicon, built on the Mac)
name the same files for each role."""
import pathlib
import re
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'


def executables(script):
    text = (SCRIPTS / script).read_text()
    found = {}
    for role in ('maker', 'taker'):
        listed = re.search(rf'^  {role}\) bins=\(([^)]*)\) ;;$', text, re.M)
        common = re.search(r'^bins\+=\(([^)]*)\)$', text, re.M)
        found[role] = sorted(listed.group(1).split() + common.group(1).split())
    return found


class RolePackages(unittest.TestCase):
    def test_linux_and_darwin_packages_carry_the_same_executables(self):
        linux = executables('package-role-node.sh')
        darwin = executables('package-role-node-darwin.sh')
        self.assertEqual(linux, darwin)
        self.assertIn('lez-maker-node', linux['maker'])
        self.assertNotIn('lez-taker-node', linux['maker'])
        self.assertIn('lez-v02-bridge-poc', linux['taker'])


if __name__ == '__main__':
    unittest.main()
