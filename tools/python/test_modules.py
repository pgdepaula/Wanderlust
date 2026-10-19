"""Executable-resolution regressions independent of installed LLVM binaries."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import modules


class LldResolution(unittest.TestCase):
    def test_path_lookup_retains_windows_executable_suffix(self):
        located = r'C:\Program Files\LLVM\bin\ld.lld.exe'
        with patch.dict(os.environ, {}, clear=True), patch('modules.shutil.which', return_value=located):
            self.assertEqual(modules.find_lld(), [located])

    def test_explicit_override_resolves_to_actual_executable(self):
        with patch.dict(os.environ, {'WL_LLD': 'custom-lld'}, clear=True), patch('modules.shutil.which', return_value='/opt/llvm/custom-lld'):
            self.assertEqual(modules.find_lld(), ['/opt/llvm/custom-lld'])

    def test_rustup_fallback_selects_host_and_exact_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            host = 'x86_64-pc-windows-msvc'
            binary = Path(directory, 'lib', 'rustlib', host, 'bin', 'rust-lld.exe')
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'fixture')
            binary.with_suffix('.exe.manifest').write_text('must not be selected')
            replies = [subprocess.CompletedProcess([], 0, directory), subprocess.CompletedProcess([], 0, f'rustc fixture\nhost: {host}\n')]
            with patch.dict(os.environ, {}, clear=True), patch('modules.shutil.which', return_value=None), patch('modules.sys.platform', 'win32'), patch('modules.subprocess.run', side_effect=replies):
                self.assertEqual(modules.find_lld(), [str(binary), '-flavor', 'gnu'])

    def test_missing_host_linker_fails_instead_of_using_another_target(self):
        with tempfile.TemporaryDirectory() as directory:
            other = Path(directory, 'lib', 'rustlib', 'aarch64-unknown-linux-gnu', 'bin', 'rust-lld')
            other.parent.mkdir(parents=True)
            other.write_bytes(b'wrong target')
            replies = [subprocess.CompletedProcess([], 0, directory), subprocess.CompletedProcess([], 0, 'host: x86_64-unknown-linux-gnu\n')]
            with patch.dict(os.environ, {}, clear=True), patch('modules.shutil.which', return_value=None), patch('modules.subprocess.run', side_effect=replies):
                with self.assertRaises(SystemExit):
                    modules.find_lld()


if __name__ == '__main__':
    unittest.main()
