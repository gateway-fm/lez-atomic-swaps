"""from-scratch.sh on its two host architectures.

The script builds the devnet on arm64 hosts and on x86_64 Linux. Nothing here
runs a build: the tests read the script and the builder image's Dockerfile and
hold the per-architecture pins of the two to each other.
"""
import pathlib
import re
import unittest

DEPLOY = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = (DEPLOY / 'scripts' / 'from-scratch.sh').read_text()
DOCKERFILE = (DEPLOY / 'builder' / 'Dockerfile').read_text()


def dockerfile_arg(name):
    return re.search(rf'^ARG {name}=(\S+)$', DOCKERFILE, re.M).group(1)


class FromScratchHost(unittest.TestCase):
    def test_rapidsnark_pins_are_the_builder_images(self):
        arms = dict(re.findall(
            r'RAPIDSNARK_URL="\$RAPIDSNARK_RELEASE/(rapidsnark-linux-\w+-pic-v[\d.]+\.zip)"\n'
            r'\s+readonly RAPIDSNARK_LIB_SHA256="([0-9a-f]{64})"', SCRIPT))
        self.assertEqual(arms, {
            'rapidsnark-linux-x86_64-pic-v0.0.8.zip': dockerfile_arg('RAPIDSNARK_AMD64_LIB_SHA256'),
            'rapidsnark-linux-aarch64-pic-v0.0.8.zip': dockerfile_arg('RAPIDSNARK_ARM64_LIB_SHA256'),
        })
        for asset in arms:
            self.assertIn(asset, DOCKERFILE)
        release = re.search(r'^readonly RAPIDSNARK_RELEASE="(\S+)"$', SCRIPT, re.M).group(1)
        self.assertEqual(release, dockerfile_arg('RAPIDSNARK_RELEASE'))

    def test_the_host_phase_admits_x86_64_linux_and_arm64(self):
        admitted = re.search(r'case "\$\(uname -s\)-\$\(uname -m\)" in\n\s+(\S+)\) ;;', SCRIPT).group(1)
        self.assertEqual(set(admitted.split('|')),
                         {'Darwin-arm64', 'Linux-aarch64', 'Linux-arm64', 'Linux-x86_64'})

    def test_the_guest_build_asks_for_emulation_only_off_x86_64(self):
        self.assertIn('"$(uname -m)" != x86_64 && ! -e /proc/sys/fs/binfmt_misc/qemu-x86_64', SCRIPT)

    def test_zebrad_module_is_pinned_for_both_architectures(self):
        pins = dict(re.findall(r'^readonly ZEBRAD_MODULE_SHA256_(\w+)=([0-9a-f]{64})$', SCRIPT, re.M))
        self.assertEqual(set(pins), {'AARCH64', 'X86_64'})
        self.assertNotEqual(pins['AARCH64'], pins['X86_64'])
        self.assertIn('x86_64|amd64) echo "$ZEBRAD_MODULE_SHA256_X86_64"', SCRIPT)

    def test_the_desk_suites_run_in_the_stacks_own_containers(self):
        self.assertIn('docker exec "${LEZ_STACK_PREFIX:-lez}-$role-node" node /ui-tests/verify.mjs', SCRIPT)
        self.assertNotIn('docker exec "lez-$role-node"', SCRIPT)


if __name__ == '__main__':
    unittest.main()
