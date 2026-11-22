# SPDX-License-Identifier: GPL-3.0-or-later
"""Offline tests for wbs_to_issues.py. Run: python -m unittest discover -s scripts/wbs-to-issues"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wbs_to_issues as w  # noqa: E402

LABELS = "\n".join(
    f'- {{ name: "{n}", color: "ffffff", description: "x" }}'
    for n in ["phase:P1", "epic:1A", "epic:1B", "epic:1C", "size:S", "size:M", "exec:sonnet", "exec:opus"]
)


def wp(wid, title="T", deps="—", size="S", ac="  - ok\n", executor="sonnet"):
    return (
        f"### {wid} — {title}\n- **Area:** `x`\n- **Depends on:** {deps}\n- **Size:** {size}\n"
        f"- **Executor:** {executor}\n- **Scope:**\n  - do it\n- **Interfaces:** none\n"
        f"- **Acceptance criteria:**\n{ac}- **Tests:** t\n- **Out of scope:** none\n\n"
    )


class Fixture(unittest.TestCase):
    def load(self, text):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        (root / "docs" / "wbs").mkdir(parents=True)
        (root / ".github").mkdir()
        (root / ".github" / "labels.yml").write_text(LABELS, encoding="utf-8")
        (root / "docs" / "wbs" / "P1.md").write_text(text, encoding="utf-8")
        w.ROOT, w.WBS_DIR, w.LABELS_YML = root, root / "docs" / "wbs", root / ".github" / "labels.yml"
        wps = w.load_wbs()
        return wps, w.lint(wps)


class ParseTests(unittest.TestCase):
    def test_parse_deps_forms(self):
        wps, epics = w.parse_deps("WP-1A.01, 5B.02–5B.04, E1C (note), epic 1B, 01-architecture §13")
        self.assertEqual(wps, ["WP-1A.01", "WP-5B.02", "WP-5B.03", "WP-5B.04"])
        self.assertEqual(epics, ["1C", "1B"])
        self.assertEqual(w.parse_deps("—"), ([], []))
        self.assertEqual(w.parse_deps("WP-1A.01 (see WP-9Z.99)"), (["WP-1A.01"], []))


class LintTests(Fixture):
    def test_valid(self):
        wps, errs = self.load(wp("WP-1A.01") + wp("WP-1A.02", deps="WP-1A.01"))
        self.assertEqual(errs, [])
        self.assertEqual(wps[1]["depends"], ["WP-1A.01"])

    def test_epic_expansion(self):
        wps, errs = self.load(
            wp("WP-1B.01") + wp("WP-1B.02") + wp("WP-1B.03") + wp("WP-1A.01", deps="E1B") + wp("WP-1B.04", deps="E1B")
        )
        self.assertEqual(errs, [])
        by = {x["id"]: x for x in wps}
        self.assertEqual(by["WP-1A.01"]["depends"], ["WP-1B.01", "WP-1B.02", "WP-1B.03", "WP-1B.04"])
        self.assertNotIn("WP-1B.04", by["WP-1B.04"]["depends"])  # an epic dep never includes the WP itself

    def test_cycle_detected(self):
        _, errs = self.load(wp("WP-1A.01", deps="WP-1A.02") + wp("WP-1A.02", deps="WP-1A.03") + wp("WP-1A.03", deps="WP-1A.01"))
        self.assertTrue(any("dependency cycle (3 WPs)" in e for e in errs), errs)

    def test_cycle_through_epic_expansion(self):
        _, errs = self.load(wp("WP-1A.01", deps="E1B") + wp("WP-1B.01", deps="E1A"))
        self.assertTrue(any("dependency cycle" in e for e in errs), errs)

    def test_unknown_id_and_epic(self):
        _, errs = self.load(wp("WP-1A.01", deps="WP-1A.99, E1C"))
        self.assertTrue(any("unknown id WP-1A.99" in e for e in errs), errs)
        self.assertTrue(any("epic E1C" in e for e in errs), errs)

    def test_size_and_acceptance(self):
        _, errs = self.load(wp("WP-1A.01", size="L", ac=""))
        self.assertTrue(any("not S or M" in e for e in errs), errs)
        self.assertTrue(any("empty acceptance" in e for e in errs), errs)

    def test_bad_id_and_duplicates(self):
        _, errs = self.load(wp("WP-1A.1") + wp("WP-1A.02") + wp("WP-1A.02"))
        self.assertTrue(any("does not match" in e for e in errs), errs)
        self.assertTrue(any("duplicate id WP-1A.02" in e for e in errs), errs)

    def test_missing_label(self):
        _, errs = self.load(wp("WP-1A.01").replace("WP-1A.01", "WP-1D.01"))
        self.assertTrue(any("epic:1D" in e for e in errs), errs)


class BodyTests(Fixture):
    def test_body_marker_checklist_and_stability(self):
        wps, _ = self.load(wp("WP-1A.01") + wp("WP-1A.02", deps="WP-1A.01"))
        titles = {x["id"]: x["title"] for x in wps}
        b1 = w.render_body(wps[1], {"WP-1A.01": 7}, {}, titles)
        self.assertIn("<!-- wp:WP-1A.02 -->", b1)
        self.assertIn("- [ ] #7 WP-1A.01", b1)
        self.assertEqual(b1, w.render_body(wps[1], {"WP-1A.01": 7}, {}, titles))
        wps[1]["acceptance"] = ["changed"]
        b2 = w.render_body(wps[1], {"WP-1A.01": 7}, {}, titles)
        self.assertNotEqual(w.HASH_RE.search(b1).group(1), w.HASH_RE.search(b2).group(1))


if __name__ == "__main__":
    unittest.main()
