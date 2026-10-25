"""Verify USB selection without opening or resetting any real device."""
import importlib.machinery
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader("board_tool", str(Path(__file__).parents[1] / "tools/board"))
spec = importlib.util.spec_from_loader(loader.name, loader)
board = importlib.util.module_from_spec(spec)
loader.exec_module(board)


class BoardSelection(unittest.TestCase):
    def test_gamma_is_excluded_by_product(self):
        ports = [("/dev/ttyACM0", board.RHAPD, "00")]
        with patch.object(board, "ports", return_value=ports), patch.object(board, "read", return_value="Bitaxe Gamma RHAP-D"):
            self.assertIsNone(board.find(board.RHAPD))

    def test_bonanza_control_port_is_selected(self):
        ports = [("/dev/ttyACM0", board.RHAPD, "00"), ("/dev/ttyACM1", board.RHAPD, "02")]
        with patch.object(board, "ports", return_value=ports), patch.object(board, "read", return_value=board.RHAPD_PRODUCT):
            self.assertEqual(board.find(board.RHAPD), "/dev/ttyACM0")

    def test_multiple_bonanzas_are_rejected(self):
        ports = [("/dev/ttyACM0", board.RHAPD, "00"), ("/dev/ttyACM2", board.RHAPD, "00")]
        with patch.object(board, "ports", return_value=ports), patch.object(board, "read", return_value=board.RHAPD_PRODUCT), patch("sys.stderr"):
            with self.assertRaises(SystemExit):
                board.find(board.RHAPD)

    def test_existing_rom_and_running_bonanza_are_rejected(self):
        with patch.object(board, "find", side_effect=["/dev/ttyACM0", "/dev/ttyACM2"]), patch("sys.stderr"):
            with self.assertRaises(SystemExit):
                board.bootloader()


if __name__ == "__main__":
    unittest.main()
