"""Rust alignment preserves the instruction comparison's established tie order."""
import difflib
import random
import unittest

from fujimod.corpus import align_sequences


def reference(left, right):
    result = []
    for _, i, end_i, j, end_j in difflib.SequenceMatcher(None, left, right, autojunk=False).get_opcodes():
        result.extend((i + offset if i + offset < end_i else None,
                       j + offset if j + offset < end_j else None)
                      for offset in range(max(end_i - i, end_j - j)))
    return result


class AlignmentTests(unittest.TestCase):
    def test_repeated_and_unequal_instruction_sequences(self):
        rng = random.Random(7131)
        for _ in range(500):
            left = [rng.choice(('load', 'store', 'branch', 'return')) for _ in range(rng.randrange(30))]
            right = [rng.choice(('load', 'store', 'branch', 'return')) for _ in range(rng.randrange(30))]
            self.assertEqual(align_sequences(left, right), reference(left, right))

    def test_common_instructions_are_never_treated_as_junk(self):
        left = ['load'] * 300 + ['branch', 'return']
        right = ['store'] + ['load'] * 300 + ['return']
        self.assertEqual(align_sequences(left, right), reference(left, right))


if __name__ == '__main__':
    unittest.main()
