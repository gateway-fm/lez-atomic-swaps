"""Argument contract of the native desk bridge; no Docker and no sockets."""
import importlib.util
import io
import contextlib
import pathlib
import sys
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1] / 'scripts'


def module():
    spec = importlib.util.spec_from_file_location('desk_sockets', SCRIPTS / 'desk-sockets.py')
    obj = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(obj)
    return obj


desk_sockets = module()


def parse(*argv):
    """Runs main() up to the point it would serve, and returns (roles, prefix)."""
    served = {}
    original_argv, original_serve = sys.argv, desk_sockets.serve

    def capture(directory, prefix, roles):
        served.update(directory=directory, prefix=prefix, roles=tuple(roles))

    def run(coroutine):
        # `serve` is replaced above and returns nothing, so there is no
        # coroutine to await; this keeps main() from reaching the event loop.
        if coroutine is not None:
            coroutine.close()

    desk_sockets.serve = capture
    desk_sockets.asyncio.run = run
    sys.argv = ['desk-sockets.py', *argv]
    try:
        desk_sockets.main()
    finally:
        sys.argv, desk_sockets.serve = original_argv, original_serve
    return served


class DeskSocketArguments(unittest.TestCase):
    def test_no_roles_serves_both(self):
        """The command the native setup guide prints takes no role arguments."""
        self.assertEqual(parse()['roles'], ('maker', 'taker'))

    def test_one_role_serves_that_role(self):
        for role in ('maker', 'taker'):
            with self.subTest(role=role):
                self.assertEqual(parse(role)['roles'], (role,))

    def test_both_roles_spelled_out(self):
        self.assertEqual(parse('maker', 'taker')['roles'], ('maker', 'taker'))

    def test_prefix_without_roles_still_serves_both(self):
        served = parse('--prefix', 'r22-lez')
        self.assertEqual(served['roles'], ('maker', 'taker'))
        self.assertEqual(served['prefix'], 'r22-lez')

    def test_unknown_role_is_refused(self):
        stderr = io.StringIO()
        with self.assertRaises(SystemExit) as refusal, contextlib.redirect_stderr(stderr):
            parse('bogus')
        self.assertEqual(refusal.exception.code, 2)
        self.assertIn('bogus', stderr.getvalue())

    def test_unknown_role_beside_a_known_one_is_refused(self):
        stderr = io.StringIO()
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(stderr):
            parse('maker', 'bogus')
        self.assertIn('bogus', stderr.getvalue())


if __name__ == '__main__':
    unittest.main()
