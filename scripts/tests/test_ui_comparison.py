import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location("ui_comparison", Path(__file__).parents[1] / "ui_comparison.py")
comparison = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparison)


class CaptureAcceptance(unittest.TestCase):
    def scene(self, **changes):
        return {"originalWidth": 900, "originalHeight": 660, "width": 900, "height": 660,
                "originalHasContent": True, "candidateHasContent": True,
                "changedPixels": 1200, "meanChannelError": .05, "identical": False, **changes}

    def test_raster_differences_do_not_change_the_exact_pixel_result(self):
        record = self.scene()
        self.assertEqual(comparison.comparison_errors({"light": record}, 1, require_content=True), [])
        self.assertFalse(record["identical"])
        self.assertTrue(comparison.comparison_errors({"light": record}, 1, strict_pixels=True))

    def test_missing_scenes_and_failed_captures_are_rejected(self):
        for records in [{}, {"light": {"error": "capture failed"}}, {"status": "inconclusive"}]:
            with self.subTest(records=records):
                self.assertTrue(comparison.comparison_errors(records, 1))

    def test_equal_area_does_not_hide_different_window_dimensions(self):
        record = self.scene(width=660, height=900, changedPixels=0, meanChannelError=0)
        self.assertTrue(comparison.comparison_errors({"light": record}, 1))

    def test_blank_or_unverified_content_is_rejected(self):
        for field in ["originalHasContent", "candidateHasContent"]:
            for value in [False, None]:
                self.assertTrue(comparison.comparison_errors({"light": self.scene(**{field: value})},
                                                             1, require_content=True))

    def test_large_or_invalid_color_errors_are_rejected(self):
        for value in [2.5001, -1, float("nan"), float("inf"), None, "0"]:
            with self.subTest(value=value):
                self.assertTrue(comparison.comparison_errors({"light": self.scene(meanChannelError=value)}, 1))

    def test_invalid_pixel_counts_and_dimensions_are_rejected(self):
        for change in [{"changedPixels": -1}, {"changedPixels": 900 * 660 + 1},
                       {"width": 0}, {"width": True}]:
            self.assertTrue(comparison.comparison_errors({"light": self.scene(**change)}, 1))


if __name__ == "__main__":
    unittest.main()
