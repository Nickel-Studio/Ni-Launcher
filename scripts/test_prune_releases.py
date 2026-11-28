import unittest

from prune_releases import parse_version, select_retention


class PruneReleaseTests(unittest.TestCase):
    def test_keeps_current_and_previous_stable_release(self):
        tags = [
            "v1.0.0",
            "v1.0.1",
            "v1.0.2",
            "v1.0.3-beta",
            "v1.0.3",
            "v1.0.4",
        ]
        kept, obsolete = select_retention(tags, "v1.0.3")
        self.assertEqual(kept, ["v1.0.3", "v1.0.2"])
        self.assertEqual(obsolete, ["v1.0.0", "v1.0.1", "v1.0.3-beta"])

    def test_ignores_future_releases_and_non_version_tags(self):
        kept, obsolete = select_retention(
            ["v1.0.2", "v1.0.3", "v1.0.4", "nightly"], "v1.0.3"
        )
        self.assertEqual(kept, ["v1.0.3", "v1.0.2"])
        self.assertEqual(obsolete, [])

    def test_requires_current_stable_tag(self):
        with self.assertRaises(ValueError):
            select_retention(["v1.0.3-beta"], "v1.0.3")
        with self.assertRaises(ValueError):
            select_retention(["v1.0.3-beta", "v1.0.3"], "v1.0.3-beta")

    def test_parses_stable_and_prerelease_versions(self):
        self.assertEqual(parse_version("v1.2.3"), ((1, 2, 3), None))
        self.assertEqual(parse_version("v1.2.3-beta.1"), ((1, 2, 3), "beta.1"))
        self.assertIsNone(parse_version("nightly"))


if __name__ == "__main__":
    unittest.main()
