"""Publishing must reject partial, empty, unexpected or already published releases."""
import copy
import unittest

from release_assets import check, metadata


class ReleaseAssetsTest(unittest.TestCase):
    def setUp(self):
        self.manifest = {"extension": {"id": "github", "version": "0.13.1",
            "platforms": ["macos-arm64", "macos-x86_64", "linux-x86_64", "linux-arm64"]}}
        self.tag = "v0.13.1"
        self.release = {"isDraft": True, "tagName": self.tag, "assets": [
            {"name": f"github-0.13.1-{p}.tar.gz", "size": 123}
            for p in self.manifest["extension"]["platforms"]]}

    def test_complete_draft_can_publish(self):
        self.release["assets"].reverse()
        check(self.manifest, self.tag, self.release, assets=True)

    def test_partial_draft_can_receive_more_uploads(self):
        self.release["assets"] = []
        check(self.manifest, self.tag, self.release)

    def test_invalid_upload_sets_cannot_publish(self):
        original = self.release["assets"]
        bad_sets = [[], original[:-1], original + [original[0]],
                    original + [{"name": "other.tar.gz", "size": 123}]]
        empty = copy.deepcopy(original)
        empty[0]["size"] = 0
        bad_sets.append(empty)
        for uploaded in bad_sets:
            with self.subTest(uploaded=uploaded), self.assertRaises(ValueError):
                check(self.manifest, self.tag, {**self.release, "assets": uploaded}, assets=True)

    def test_wrong_tag_and_published_release_are_refused(self):
        for changes in [{"isDraft": False}, {"tagName": "v0.13.0"}]:
            for assets in [False, True]:
                with self.subTest(changes=changes, assets=assets), self.assertRaises(ValueError):
                    check(self.manifest, self.tag, {**self.release, **changes}, assets=assets)
        with self.assertRaises(ValueError):
            metadata(self.manifest, "v0.13.0")

    def test_manifest_cannot_allow_an_empty_or_duplicate_asset_set(self):
        for platforms in [[], ["linux-arm64", "linux-arm64"], "linux-arm64"]:
            self.manifest["extension"]["platforms"] = platforms
            with self.subTest(platforms=platforms), self.assertRaises(ValueError):
                check(self.manifest, self.tag, {**self.release, "assets": []}, assets=True)


if __name__ == "__main__":
    unittest.main()
