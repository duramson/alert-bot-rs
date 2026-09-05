"""Exercise the real backup shell pipeline offline; never contact a server."""
import gzip
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "backup-postgres.sh"


class BackupTest(unittest.TestCase):
    def run_backup(self, failure):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "bin"
            binaries.mkdir()
            programs = {
                "pg_dump": """#!/bin/sh
printf '%s\\n' '-- SQL payload'
[ "$BACKUP_TEST_FAILURE" != dump ]
""",
                "gzip": """#!/bin/sh
if [ "$BACKUP_TEST_FAILURE" = gzip ]; then
    cat >/dev/null
    exit 2
fi
exec /usr/bin/gzip "$@"
""",
                "sshpass": "#!/bin/sh\nexit 0\n",
                "curl": """#!/usr/bin/env python3
import os, sys
from pathlib import Path
root = Path(os.environ['BACKUP_TEST_REMOTE'])
failure = os.environ['BACKUP_TEST_FAILURE']
args = sys.argv[1:]
if '--upload-file' in args:
    data = sys.stdin.buffer.read()
    (root / 'backup.part').write_bytes(data)
    if failure == 'upload':
        sys.exit(22)
if '--quote' in args:
    command = args[args.index('--quote') + 1]
    assert command.lstrip('-').startswith('rename ')
    (root / 'rename-requested').touch()
    if failure == 'rename':
        sys.exit(21)
    (root / 'backup.part').rename(root / 'backup.sql.gz')
""",
            }
            for name, program in programs.items():
                path = binaries / name
                path.write_text(program)
                path.chmod(0o755)
            env = dict(os.environ, PATH=str(binaries) + os.pathsep + os.environ["PATH"],
                       BACKUP_NETCUP_HOST="offline.invalid", BACKUP_NETCUP_USER="test",
                       BACKUP_NETCUP_PASS="test", BACKUP_NETCUP_PATH="backups/test",
                       BACKUP_TEST_REMOTE=str(root), BACKUP_TEST_FAILURE=failure)
            result = subprocess.run(["bash", str(SCRIPT)], env=env, capture_output=True, timeout=10)
            final = root / "backup.sql.gz"
            return (result.returncode, final.read_bytes() if final.exists() else None,
                    (root / "rename-requested").exists())

    def test_success_publishes_complete_dump(self):
        status, data, renamed = self.run_backup("")
        self.assertEqual(status, 0)
        self.assertTrue(renamed)
        self.assertEqual(gzip.decompress(data), b"-- SQL payload\n")

    def test_failed_pipeline_never_requests_rename(self):
        for failure in ["dump", "gzip", "upload"]:
            with self.subTest(failure=failure):
                status, data, renamed = self.run_backup(failure)
                self.assertNotEqual(status, 0)
                self.assertIsNone(data)
                self.assertFalse(renamed)

    def test_rename_failure_is_reported(self):
        status, data, renamed = self.run_backup("rename")
        self.assertNotEqual(status, 0)
        self.assertIsNone(data)
        self.assertTrue(renamed)


if __name__ == "__main__":
    unittest.main()
