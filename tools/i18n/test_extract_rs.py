#!/usr/bin/env python3
"""Tests of extract_rs.py's test-module handling (CI runs this):

    python3 tools/i18n/test_extract_rs.py
"""

import os
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import extract_rs  # noqa: E402

SOURCE = '''fn shown() { tr("Before"); }

#[cfg(test)]
mod tests {
    fn t() { tr("Only in a test"); let s = "}"; let c = '{'; }
}

fn later() { tr("After the tests"); }

#[cfg(test)]
pub(crate) mod fake {
    // A brace in a comment: }
    fn f() { trf!("Fake {x}", x = 1); }
}

fn last() { trc("ctx", "Last"); }
'''


class TestModules(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.src = os.path.join(self.dir.name, "x.rs")
        with open(self.src, "w", encoding="utf-8") as f:
            f.write(SOURCE)
        self.pot = os.path.join(self.dir.name, "x.pot")
        sys.argv = ["extract_rs.py", self.pot, self.src]
        extract_rs.main()
        with open(self.pot, encoding="utf-8") as f:
            self.out = f.read()

    def tearDown(self):
        self.dir.cleanup()

    def test_test_only_strings_are_left_out(self):
        self.assertNotIn("Only in a test", self.out)
        self.assertNotIn("Fake", self.out)

    def test_strings_after_test_modules_are_kept(self):
        for msg in ("Before", "After the tests", "Last"):
            self.assertIn(f'msgid "{msg}"', self.out)

    def test_line_numbers_stay_right(self):
        self.assertIn("x.rs:8\n", self.out)
        self.assertIn("x.rs:16\n", self.out)

    def test_blank_tests_keeps_newlines(self):
        blanked = extract_rs.blank_tests(SOURCE)
        self.assertEqual(blanked.count("\n"), SOURCE.count("\n"))


if __name__ == "__main__":
    unittest.main()
