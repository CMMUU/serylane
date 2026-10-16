import importlib.util
from pathlib import Path
import unittest
spec = importlib.util.spec_from_file_location("native_gate", Path(__file__).with_name("test-native-updater.py"))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
class Baselines(unittest.TestCase):
    def test_recent_two_and_regression_remain_strict_and_unique(self):
        releases = [{"tag_name": v} for v in ["v0.7.28", "v0.7.29", "v0.7.27", "v0.7.30", "v0.7.29-rc1", "v0.07.31", "v1.0.0?x=1"]]
        releases += [{"tag_name":"v0.8.0", "draft":True}, {"tag_name":"v0.9.0", "prerelease":True}]
        self.assertEqual(gate.select_baselines(releases, "0.7.30"), ["0.7.29", "0.7.28", "0.7.27"])
        self.assertEqual(gate.select_baselines(releases, "0.7.29"), ["0.7.28", "0.7.27"])
    def test_native_process_receives_exact_environment_contract(self):
        env = gate.install_environment(Path("archive"), Path("baseline"), "0.7.28")
        self.assertEqual(env["SERYLANE_UPDATE_ARCHIVE"], "archive")
        self.assertEqual(env["SERYLANE_UPDATE_BASELINE"], "baseline")
        self.assertEqual(env["SERYLANE_UPDATE_BASELINE_VERSION"], "0.7.28")
    def test_missing_baselines_and_invalid_candidates_fail_closed(self):
        with self.assertRaises(ValueError): gate.select_baselines([], "0.7.30")
        with self.assertRaises(ValueError): gate.select_baselines([], "0.7.30/other")
if __name__ == "__main__": unittest.main()
